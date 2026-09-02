use std::{
    collections::BTreeMap,
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use anyhow::Context;
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
    autostart,
    dashboard::{self, DashboardData},
    desktop::{self, DesktopTargets},
    error::ApiError,
    local_config, protocol,
    provider::{ModelInfo, Registry, RegistryStore},
    sources::SourceSelection,
    target_config::TargetPreferences,
    upgrade,
};

#[derive(Clone)]
struct AppState {
    registry: RegistryStore,
}

async fn anthropic_count_tokens(Json(request): Json<Value>) -> impl IntoResponse {
    let serialized = serde_json::to_string(&request).unwrap_or_default();
    Json(json!({"input_tokens": serialized.len().div_ceil(4)}))
}

async fn anthropic_messages(
    State(state): State<AppState>,
    Json(request): Json<Value>,
) -> Result<ResponseBody, ApiError> {
    let registry = state.registry.snapshot();
    let requested_model = request
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("missing 'model'"))?;
    let routable_model = requested_model
        .strip_prefix("claude-joocode/")
        .unwrap_or(requested_model);
    let (provider, upstream_model) = registry
        .resolve(routable_model)
        .map_err(|error| ApiError::not_found(error.to_string()))?;
    let chat_request = protocol::anthropic_to_chat_request(&request, &upstream_model)?;
    let (base_url, upstream_headers) = provider
        .request_parts(registry.client())
        .await
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
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
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
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
        Ok(ResponseBody::Stream(anthropic_stream_response(
            response.bytes_stream(),
            requested_model.to_owned(),
        )))
    } else {
        let chat = response
            .json::<Value>()
            .await
            .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
        Ok(ResponseBody::Json(Json(
            protocol::chat_to_anthropic_response(chat, requested_model)?,
        )))
    }
}

fn anthropic_stream_response<S>(upstream: S, requested_model: String) -> Response
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let events = stream! {
        let message_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
        yield Ok::<Bytes, Infallible>(Bytes::from(format!(
            "event: message_start\ndata: {}\n\n",
            json!({"type":"message_start","message":{"id":message_id,"type":"message","role":"assistant","model":requested_model,"content":[],"stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":0,"output_tokens":0}}})
        )));
        yield Ok(Bytes::from(format!(
            "event: content_block_start\ndata: {}\n\n",
            json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})
        )));
        let reader = StreamReader::new(upstream.map_err(std::io::Error::other));
        let mut lines = reader.lines();
        let mut output_tokens = 0_u64;
        let mut text_open = true;
        let mut tools = BTreeMap::<usize, usize>::new();
        let mut next_block = 1_usize;
        while let Ok(Some(line)) = lines.next_line().await {
            let Some(data) = line.strip_prefix("data:") else { continue; };
            let data = data.trim();
            if data == "[DONE]" { break; }
            let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue; };
            if let Some(text) = chunk.pointer("/choices/0/delta/content").and_then(Value::as_str) {
                output_tokens = output_tokens.saturating_add((text.len().div_ceil(4)) as u64);
                yield Ok(Bytes::from(format!(
                    "event: content_block_delta\ndata: {}\n\n",
                    json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}})
                )));
            }
            for call in chunk
                .pointer("/choices/0/delta/tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let upstream_index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let block_index = if let Some(block_index) = tools.get(&upstream_index) {
                    *block_index
                } else {
                    if text_open {
                        yield Ok(Bytes::from(format!(
                            "event: content_block_stop\ndata: {}\n\n",
                            json!({"type":"content_block_stop","index":0})
                        )));
                        text_open = false;
                    }
                    let block_index = next_block;
                    next_block = next_block.saturating_add(1);
                    tools.insert(upstream_index, block_index);
                    yield Ok(Bytes::from(format!(
                        "event: content_block_start\ndata: {}\n\n",
                        json!({
                            "type":"content_block_start",
                            "index":block_index,
                            "content_block":{
                                "type":"tool_use",
                                "id":call.get("id").cloned().unwrap_or_else(|| Value::String(protocol::call_id())),
                                "name":call.pointer("/function/name").cloned().unwrap_or_else(|| Value::String("tool".into())),
                                "input":{}
                            }
                        })
                    )));
                    block_index
                };
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                    && !arguments.is_empty()
                {
                    output_tokens = output_tokens.saturating_add((arguments.len().div_ceil(4)) as u64);
                    yield Ok(Bytes::from(format!(
                        "event: content_block_delta\ndata: {}\n\n",
                        json!({
                            "type":"content_block_delta",
                            "index":block_index,
                            "delta":{"type":"input_json_delta","partial_json":arguments}
                        })
                    )));
                }
            }
        }
        if text_open {
            yield Ok(Bytes::from(format!(
                "event: content_block_stop\ndata: {}\n\n",
                json!({"type":"content_block_stop","index":0})
            )));
        }
        for block_index in tools.values() {
            yield Ok(Bytes::from(format!(
                "event: content_block_stop\ndata: {}\n\n",
                json!({"type":"content_block_stop","index":block_index})
            )));
        }
        yield Ok(Bytes::from(format!(
            "event: message_delta\ndata: {}\n\n",
            json!({"type":"message_delta","delta":{"stop_reason":if tools.is_empty() {"end_turn"} else {"tool_use"},"stop_sequence":null},"usage":{"output_tokens":output_tokens}})
        )));
        yield Ok(Bytes::from("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
    };
    Response::builder()
        .header(header::CONTENT_TYPE, "text/event-stream")
        .header(header::CACHE_CONTROL, "no-cache")
        .body(Body::from_stream(events))
        .expect("valid Anthropic streaming response")
}

struct PersistentProxyHandoff {
    active: bool,
}

impl PersistentProxyHandoff {
    fn begin(interactive: bool) -> anyhow::Result<Self> {
        if interactive {
            autostart::prepare_dashboard_handoff()
                .context("failed to prepare the persistent Joocode proxy handoff")?;
        }
        Ok(Self {
            active: interactive,
        })
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for PersistentProxyHandoff {
    fn drop(&mut self) {
        if self.active {
            let _ = autostart::resume_detached();
        }
    }
}

fn desktop_base_url(address: std::net::SocketAddr) -> String {
    let host = match address.ip() {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_owned(),
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_owned(),
        std::net::IpAddr::V6(ip) => format!("[{ip}]"),
        ip => ip.to_string(),
    };
    format!("http://{host}:{}/v1", address.port())
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
    let PreparedServer::Ready {
        listener,
        app,
        address,
        port_warning,
    } = prepare_server(host, port, RegistryStore::new(registry)).await?
    else {
        tracing::info!(port, "Joocode is already running in the background");
        return Ok(());
    };
    if let Some(warning) = port_warning {
        tracing::warn!("{warning}");
    }
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
    base_url: Option<String>,
) -> anyhow::Result<()> {
    let interactive = dashboard::is_interactive();
    let mut persistent_proxy = PersistentProxyHandoff::begin(interactive)?;
    let registry_store = RegistryStore::new(registry.clone());
    let PreparedServer::Ready {
        listener,
        app,
        address,
        port_warning,
    } = prepare_server(host, port, registry_store.clone()).await?
    else {
        persistent_proxy.disarm();
        println!("Joocode is already running in the background at http://{host}:{port}.");
        return Ok(());
    };
    let base_url = base_url.unwrap_or_else(|| desktop_base_url(address));
    let dashboard_data = DashboardData::new(&registry, &targets, address, port_warning);
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(async move {
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

    if !interactive {
        info!(address = %dashboard_data.listening, "joocode listening");
        shutdown_signal().await;
        let _ = shutdown_tx.send(());
        server.await??;
        return Ok(());
    }

    let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let shutdown_event_tx = event_tx.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            let _ = shutdown_event_tx.send(dashboard::DashboardEvent::ShutdownRequested);
        }
    });
    let update_event_tx = event_tx.clone();
    tokio::spawn(async move {
        if let Ok(Some(tag)) = upgrade::check().await {
            let _ = update_event_tx.send(dashboard::DashboardEvent::UpdateAvailable(tag));
        }
    });
    let reload_store = registry_store;
    let reload_targets = targets.clone();
    let reload_base_url = base_url.clone();
    tokio::spawn(async move {
        let mut active_targets = reload_targets;
        while let Some(command) = command_rx.recv().await {
            match command {
                dashboard::DashboardCommand::AddProvider { base_url, api_key } => {
                    let result = async {
                        let client = reload_store.snapshot().client().clone();
                        let provider = local_config::probe(&client, &base_url, &api_key).await?;
                        local_config::save(provider.clone())?;
                        let registry = Registry::discover(&selection).await?;
                        reload_store.replace(registry.clone());
                        let setup_registry = registry.clone();
                        let setup_targets = active_targets.clone();
                        let setup_base_url = reload_base_url.clone();
                        std::thread::spawn(move || {
                            desktop::configure_detected(
                                &setup_registry,
                                &setup_base_url,
                                &setup_targets,
                            );
                        });
                        Ok::<_, anyhow::Error>((provider, registry))
                    }
                    .await;
                    let event = match result {
                        Ok((provider, registry)) => dashboard::DashboardEvent::ProviderAdded {
                            provider: provider.name,
                            config_sources: dashboard::config_sources(&registry),
                            model_count: registry.models().len(),
                            provider_count: registry.provider_count(),
                            providers: local_config::summaries().unwrap_or_default(),
                        },
                        Err(error) => dashboard::DashboardEvent::ProviderError(error.to_string()),
                    };
                    let _ = event_tx.send(event);
                }
                dashboard::DashboardCommand::RemoveProvider { name } => {
                    let result = async {
                        local_config::remove(&name)?;
                        let registry = Registry::discover(&selection).await?;
                        reload_store.replace(registry.clone());
                        let setup_registry = registry.clone();
                        let setup_targets = active_targets.clone();
                        let setup_base_url = reload_base_url.clone();
                        std::thread::spawn(move || {
                            desktop::configure_detected(
                                &setup_registry,
                                &setup_base_url,
                                &setup_targets,
                            );
                        });
                        Ok::<_, anyhow::Error>(registry)
                    }
                    .await;
                    let event = match result {
                        Ok(registry) => dashboard::DashboardEvent::ProviderRemoved {
                            config_sources: dashboard::config_sources(&registry),
                            model_count: registry.models().len(),
                            provider_count: registry.provider_count(),
                            providers: local_config::summaries().unwrap_or_default(),
                        },
                        Err(error) => dashboard::DashboardEvent::ProviderError(error.to_string()),
                    };
                    let _ = event_tx.send(event);
                }
                dashboard::DashboardCommand::ToggleAutoStart => {
                    let event = match tokio::task::spawn_blocking(autostart::toggle_for_dashboard)
                        .await
                    {
                        Ok(Ok(status)) => dashboard::DashboardEvent::AutoStartUpdated(status),
                        Ok(Err(error)) => {
                            dashboard::DashboardEvent::ProviderError(error.to_string())
                        }
                        Err(error) => dashboard::DashboardEvent::ProviderError(error.to_string()),
                    };
                    let _ = event_tx.send(event);
                }
                dashboard::DashboardCommand::ToggleProxyTarget { target } => {
                    let enabled = !active_targets.enabled(target);
                    let registry = reload_store.snapshot();
                    let result = tokio::task::spawn_blocking({
                        let base_url = reload_base_url.clone();
                        move || {
                            desktop::configure_target(&registry, &base_url, target, enabled)?;
                            TargetPreferences::set(target, enabled)?;
                            Ok::<_, anyhow::Error>(())
                        }
                    })
                    .await;
                    let event = match result {
                        Ok(Ok(())) => {
                            active_targets.set(target, enabled);
                            dashboard::DashboardEvent::ProxyTargetUpdated { target, enabled }
                        }
                        Ok(Err(error)) => {
                            dashboard::DashboardEvent::ProviderError(error.to_string())
                        }
                        Err(error) => dashboard::DashboardEvent::ProviderError(error.to_string()),
                    };
                    let _ = event_tx.send(event);
                }
                dashboard::DashboardCommand::InstallUpdate { tag } => {
                    let event = match upgrade::install(&tag).await {
                        Ok(_) => dashboard::DashboardEvent::UpdateInstalled,
                        Err(error) => dashboard::DashboardEvent::ProviderError(format!(
                            "Update failed: {error:#}"
                        )),
                    };
                    let _ = event_tx.send(event);
                }
            }
        }
    });

    let dashboard =
        tokio::task::spawn_blocking(move || dashboard::run(dashboard_data, command_tx, event_rx));
    let dashboard_result = dashboard.await?;
    let _ = shutdown_tx.send(());
    // Desktop clients commonly keep SSE/HTTP connections open. Do not hold the
    // user's terminal indefinitely while Axum waits for those connections to
    // close after Ctrl+C/Esc. Give active requests a short grace period, then
    // drop the listener task so the background proxy handoff can proceed.
    match tokio::time::timeout(Duration::from_millis(250), &mut server).await {
        Ok(result) => result??,
        Err(_) => {
            server.abort();
            let _ = server.await;
        }
    }
    match dashboard_result? {
        dashboard::DashboardExit::Quit => {
            autostart::resume_detached()?;
            persistent_proxy.disarm();
            println!(
                "Joocode is still running in the background. You can stop it with `jcx stop`."
            );
            Ok(())
        }
        dashboard::DashboardExit::Restart => {
            let result = upgrade::restart_current();
            if result.is_ok() {
                persistent_proxy.disarm();
            }
            result
        }
    }
}

async fn prepare_server(
    host: IpAddr,
    port: u16,
    registry: RegistryStore,
) -> anyhow::Result<PreparedServer> {
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/api/hello", get(healthz))
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/messages/count_tokens", post(anthropic_count_tokens))
        .with_state(AppState { registry })
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());
    match bind_available(host, port).await? {
        BindResult::Bound {
            listener,
            port_warning,
        } => {
            let address = listener.local_addr()?;
            Ok(PreparedServer::Ready {
                listener,
                app,
                address,
                port_warning,
            })
        }
        BindResult::ExistingJoocode => Ok(PreparedServer::ExistingJoocode),
    }
}

enum PreparedServer {
    Ready {
        listener: tokio::net::TcpListener,
        app: Router,
        address: SocketAddr,
        port_warning: Option<String>,
    },
    ExistingJoocode,
}

enum BindResult {
    Bound {
        listener: tokio::net::TcpListener,
        port_warning: Option<String>,
    },
    ExistingJoocode,
}

async fn bind_available(host: IpAddr, requested_port: u16) -> std::io::Result<BindResult> {
    let requested_address = SocketAddr::from((host, requested_port));
    match tokio::net::TcpListener::bind(requested_address).await {
        Ok(listener) => Ok(BindResult::Bound {
            listener,
            port_warning: None,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            if joocode_is_running(host, requested_port).await {
                return Ok(BindResult::ExistingJoocode);
            }
            let listener = if requested_port == u16::MAX {
                tokio::net::TcpListener::bind(SocketAddr::from((host, 0))).await?
            } else {
                let mut listener = None;
                for port in requested_port + 1..=u16::MAX {
                    match tokio::net::TcpListener::bind(SocketAddr::from((host, port))).await {
                        Ok(candidate) => {
                            listener = Some(candidate);
                            break;
                        }
                        Err(candidate_error)
                            if candidate_error.kind() == std::io::ErrorKind::AddrInUse => {}
                        Err(candidate_error) => return Err(candidate_error),
                    }
                }
                match listener {
                    Some(listener) => listener,
                    None => tokio::net::TcpListener::bind(SocketAddr::from((host, 0))).await?,
                }
            };
            let actual_port = listener.local_addr()?.port();
            Ok(BindResult::Bound {
                listener,
                port_warning: Some(format!(
                    "Port {requested_port} already in used, close another process first. Using port {actual_port}."
                )),
            })
        }
        Err(error) => Err(error),
    }
}

async fn joocode_is_running(host: IpAddr, port: u16) -> bool {
    let host = match host {
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_owned(),
        IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_owned(),
        IpAddr::V6(ip) => format!("[{ip}]"),
        ip => ip.to_string(),
    };
    let Ok(client) = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_millis(500))
        .build()
    else {
        return false;
    };
    let Ok(response) = client
        .get(format!("http://{host}:{port}/api/hello"))
        .send()
        .await
    else {
        return false;
    };
    if response
        .headers()
        .get("x-joocode-service")
        .and_then(|value| value.to_str().ok())
        == Some("joocode")
    {
        return true;
    }
    response
        .json::<Value>()
        .await
        .is_ok_and(|body| body.get("ok").and_then(Value::as_bool) == Some(true))
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
async fn healthz() -> impl IntoResponse {
    (
        [
            ("x-joocode-service", "joocode"),
            ("x-joocode-version", env!("CARGO_PKG_VERSION")),
        ],
        Json(json!({
            "ok": true,
            "service": "joocode",
            "version": env!("CARGO_PKG_VERSION")
        })),
    )
}

async fn models(State(state): State<AppState>, headers: HeaderMap) -> impl IntoResponse {
    let registry = state.registry.snapshot();
    let local_credential = |name: header::HeaderName| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value == "joocode-local" || value == "Bearer joocode-local")
    };
    let anthropic = headers.contains_key("anthropic-version")
        || local_credential(header::AUTHORIZATION)
        || local_credential(header::HeaderName::from_static("x-api-key"));
    let data = registry
        .models()
        .iter()
        .map(|model| {
            let mut value = model_json(model);
            if anthropic {
                value["id"] = Value::String(format!("claude-joocode/{}", model.id));
                value["display_name"] = Value::String(model.id.clone());
            }
            value
        })
        .collect::<Vec<_>>();
    Json(json!({ "object": "list", "data": data }))
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

    #[test]
    fn desktop_base_url_follows_the_listener() {
        assert_eq!(
            desktop_base_url("127.0.0.1:18125".parse().unwrap()),
            "http://127.0.0.1:18125/v1"
        );
        assert_eq!(
            desktop_base_url("0.0.0.0:10100".parse().unwrap()),
            "http://127.0.0.1:10100/v1"
        );
    }

    #[tokio::test]
    async fn busy_port_falls_back_to_the_next_available_port() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let requested_port = occupied.local_addr().unwrap().port();
        let BindResult::Bound {
            listener,
            port_warning: warning,
        } = bind_available("127.0.0.1".parse().unwrap(), requested_port)
            .await
            .unwrap()
        else {
            panic!("a non-Joocode process must use a fallback port");
        };
        let actual_port = listener.local_addr().unwrap().port();

        assert_ne!(actual_port, requested_port);
        assert_eq!(actual_port, requested_port + 1);
        assert_eq!(
            warning.unwrap(),
            format!(
                "Port {requested_port} already in used, close another process first. Using port {actual_port}."
            )
        );
    }

    #[tokio::test]
    async fn busy_port_owned_by_joocode_reuses_the_existing_instance() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = Router::new().route("/api/hello", get(healthz));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let result = bind_available("127.0.0.1".parse().unwrap(), port)
            .await
            .unwrap();

        assert!(matches!(result, BindResult::ExistingJoocode));
        server.abort();
    }
}
