use std::{
    convert::Infallible,
    net::{IpAddr, SocketAddr},
};

use async_stream::stream;
use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use bytes::Bytes;
use futures_util::{Stream, TryStreamExt};
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use tokio_util::io::StreamReader;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing::info;

use crate::{
    dashboard::{self, DashboardData},
    desktop::{self, DesktopTargets},
    error::ApiError,
    local_config, protocol,
    provider::{ModelInfo, Registry, RegistryStore},
    sources::SourceSelection,
};

#[derive(Clone)]
struct AppState {
    registry: RegistryStore,
}

/// Zed's OpenAI-compatible provider uses Chat Completions directly. Qualified
/// models are translated only at routing time; the upstream already speaks this
/// wire format, so its JSON and SSE response can pass through unchanged.
async fn chat_completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(mut request): Json<Value>,
) -> Result<ResponseBody, ApiError> {
    let registry = state.registry.snapshot();
    let requested_model = request
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("missing 'model'"))?
        .to_owned();
    let (provider, upstream_model) = registry
        .resolve(&requested_model)
        .map_err(|e| ApiError::not_found(e.to_string()))?;
    request["model"] = Value::String(upstream_model);
    let (base_url, mut upstream_headers) = provider
        .request_parts(registry.client())
        .await
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    if let Some(value) = headers.get("x-joocode-api-key") {
        upstream_headers.insert("x-joocode-api-key", value.clone());
    }
    let response = registry
        .client()
        .post(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .headers(upstream_headers)
        .json(&request)
        .send()
        .await
        .map_err(|e| ApiError::upstream(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let status = response.status();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    Ok(ResponseBody::Stream(
        builder
            .body(Body::from_stream(stream))
            .expect("valid chat completion proxy response"),
    ))
}

async fn proxy_openai(
    state: AppState,
    mut headers: HeaderMap,
    request: Value,
) -> Result<ResponseBody, ApiError> {
    let registry = state.registry.snapshot();
    let chatgpt_session = headers.contains_key("chatgpt-account-id");
    let url = if chatgpt_session {
        "https://chatgpt.com/backend-api/codex/responses"
    } else {
        "https://api.openai.com/v1/responses"
    };
    for name in [header::HOST, header::CONTENT_LENGTH, header::CONTENT_TYPE] {
        headers.remove(name);
    }
    let response = registry
        .client()
        .post(url)
        .headers(headers)
        .json(&request)
        .send()
        .await
        .map_err(|e| ApiError::upstream(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let status = response.status();
    let content_type = response.headers().get(header::CONTENT_TYPE).cloned();
    let stream = response.bytes_stream().map_err(std::io::Error::other);
    let mut builder = Response::builder().status(status);
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    Ok(ResponseBody::Stream(
        builder
            .body(Body::from_stream(stream))
            .expect("valid OpenAI proxy response"),
    ))
}

pub async fn serve(host: IpAddr, port: u16, registry: Registry) -> anyhow::Result<()> {
    let (listener, app, address) = prepare_server(host, port, RegistryStore::new(registry)).await?;
    info!(%address, "joocode listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

pub async fn serve_dashboard(
    host: IpAddr,
    port: u16,
    registry: Registry,
    selection: SourceSelection,
    targets: DesktopTargets,
    base_url: String,
) -> anyhow::Result<()> {
    let registry_store = RegistryStore::new(registry.clone());
    let (listener, app, address) = prepare_server(host, port, registry_store.clone()).await?;
    let dashboard_data = DashboardData::new(&registry, &targets, address);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            })
            .await
    });

    let setup_registry = registry;
    let setup_targets = targets.clone();
    let setup_base_url = base_url.clone();
    std::thread::spawn(move || {
        desktop::configure_detected(&setup_registry, &setup_base_url, &setup_targets);
    });

    if !dashboard::is_interactive() {
        info!(address = %dashboard_data.listening, "joocode listening");
        shutdown_signal().await;
        let _ = shutdown_tx.send(());
        server.await??;
        return Ok(());
    }

    let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let reload_store = registry_store;
    let reload_targets = targets.clone();
    let reload_base_url = base_url.clone();
    tokio::spawn(async move {
        while let Some(dashboard::DashboardCommand::AddProvider { base_url, api_key }) =
            command_rx.recv().await
        {
            let result = async {
                let client = reload_store.snapshot().client().clone();
                let provider = local_config::probe(&client, &base_url, &api_key).await?;
                local_config::save(provider.clone())?;
                let registry = Registry::discover(&selection).await?;
                reload_store.replace(registry.clone());
                let setup_registry = registry.clone();
                let setup_targets = reload_targets.clone();
                let setup_base_url = reload_base_url.clone();
                std::thread::spawn(move || {
                    desktop::configure_detected(&setup_registry, &setup_base_url, &setup_targets);
                });
                Ok::<_, anyhow::Error>((provider, registry))
            }
            .await;
            let event = match result {
                Ok((provider, registry)) => dashboard::DashboardEvent::ProviderAdded {
                    provider: provider.name,
                    models: provider.models,
                    config_sources: dashboard::config_sources(&registry),
                },
                Err(error) => dashboard::DashboardEvent::ProviderError(error.to_string()),
            };
            let _ = event_tx.send(event);
        }
    });

    let dashboard =
        tokio::task::spawn_blocking(move || dashboard::run(dashboard_data, command_tx, event_rx));
    let dashboard_result = dashboard.await?;
    let _ = shutdown_tx.send(());
    server.await??;
    dashboard_result
}

async fn prepare_server(
    host: IpAddr,
    port: u16,
    registry: RegistryStore,
) -> anyhow::Result<(tokio::net::TcpListener, Router, SocketAddr)> {
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(AppState { registry })
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());
    let address = SocketAddr::from((host, port));
    let listener = tokio::net::TcpListener::bind(address).await?;
    let address = listener.local_addr()?;
    Ok((listener, app, address))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
async fn healthz() -> impl IntoResponse {
    Json(json!({ "ok": true }))
}

async fn models(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.snapshot();
    Json(
        json!({ "object": "list", "data": registry.models().iter().map(model_json).collect::<Vec<_>>() }),
    )
}

fn model_json(model: &ModelInfo) -> Value {
    json!({ "id": model.id, "object": "model", "created": 0, "owned_by": model.provider,
        "name": model.name, "context_window": model.context_window, "max_output_tokens": model.max_output_tokens,
        "reasoning": model.reasoning })
}

async fn responses(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Result<ResponseBody, ApiError> {
    let requested_model = request
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("missing 'model'"))?
        .to_owned();
    if !requested_model.contains('/') {
        return proxy_openai(state, headers, request).await;
    }
    let registry = state.registry.snapshot();
    let (provider, upstream_model) = registry
        .resolve(&requested_model)
        .map_err(|e| ApiError::not_found(e.to_string()))?;
    let chat_request = protocol::to_chat_request(&request, &upstream_model)?;
    let (base_url, mut upstream_headers) = provider
        .request_parts(registry.client())
        .await
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    if let Some(value) = headers
        .get("x-joocode-api-key")
        .or_else(|| headers.get("x-joc-api-key"))
        .or_else(|| headers.get("x-open-initiative-api-key"))
    {
        upstream_headers.insert("x-joocode-api-key", value.clone());
    }
    let response = registry
        .client()
        .post(format!(
            "{}/chat/completions",
            base_url.trim_end_matches('/')
        ))
        .headers(upstream_headers)
        .json(&chat_request)
        .send()
        .await
        .map_err(|e| ApiError::upstream(StatusCode::BAD_GATEWAY, e.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "upstream request failed".into());
        return Err(ApiError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("upstream returned {status}: {body}"),
        ));
    }
    if request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Ok(ResponseBody::Stream(stream_response(
            response.bytes_stream(),
            requested_model,
        )))
    } else {
        let chat: Value = response
            .json()
            .await
            .map_err(|e| ApiError::upstream(StatusCode::BAD_GATEWAY, e.to_string()))?;
        Ok(ResponseBody::Json(Json(protocol::from_chat_response(
            chat,
            &requested_model,
            protocol::response_id(),
        )?)))
    }
}

enum ResponseBody {
    Json(Json<Value>),
    Stream(Response),
}

impl IntoResponse for ResponseBody {
    fn into_response(self) -> Response {
        match self {
            Self::Json(body) => body.into_response(),
            Self::Stream(body) => body,
        }
    }
}

fn stream_response<S>(upstream: S, requested_model: String) -> Response
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let events = stream! {
        let mut state = protocol::StreamState::new(protocol::response_id(), requested_model);
        for frame in state.created_events() { yield Ok::<Bytes, Infallible>(Bytes::from(frame)); }
        let reader = StreamReader::new(upstream.map_err(std::io::Error::other));
        let mut lines = reader.lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let Some(data) = line.strip_prefix("data:") else { continue; };
            let data = data.trim();
            if data == "[DONE]" { break; }
            let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue; };
            for frame in state.consume_chunk(&chunk) { yield Ok(Bytes::from(frame)); }
        }
        for frame in state.completed_events() { yield Ok(Bytes::from(frame)); }
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(events))
        .expect("valid streaming response")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn health_route_is_available() {
        assert_eq!(healthz().await.into_response().status(), StatusCode::OK);
    }
}
