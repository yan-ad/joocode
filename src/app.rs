use std::{
    collections::BTreeMap,
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

async fn antigravity_bridge(
    State(state): State<AppState>,
    method: Method,
    OriginalUri(uri): OriginalUri,
    headers: HeaderMap,
    body: AxumBytes,
) -> Result<Response, ApiError> {
    let registry = state.registry.snapshot();
    antigravity::handle(&registry, method, uri, headers, body).await
}

async fn send_routed<F>(
    registry: &Registry,
    requested_model: &str,
    retry_policy: &upstream::RetryPolicy,
    runtime: &upstream::Runtime,
    local_key: Option<HeaderValue>,
    make_body: F,
) -> Result<upstream::UpstreamResponse, ApiError>
where
    F: Fn(&str) -> Result<Value, ApiError>,
{
    let candidates = registry
        .resolve_candidates(requested_model)
        .map_err(|error| ApiError::not_found(error.to_string()))?;
    let candidate_count = candidates.len();
    let mut last_error = None;
    for (index, (provider_key, provider, upstream_model)) in candidates.into_iter().enumerate() {
        let body = make_body(&upstream_model)?;
        let (base_url, mut headers) = match provider.request_parts(registry.client()).await {
            Ok(parts) => parts,
            Err(error) if index + 1 < candidate_count => {
                last_error = Some(error.to_string());
                continue;
            }
            Err(error) => {
                return Err(ApiError::upstream(
                    StatusCode::BAD_GATEWAY,
                    error.to_string(),
                ));
            }
        };
        if let Some(value) = &local_key {
            headers.insert("x-joocode-api-key", value.clone());
        }
        let url = format!("{}/chat/completions", base_url.trim_end_matches('/'));
        let response = match upstream::send_json(
            registry.client(),
            &url,
            &headers,
            &body,
            retry_policy,
            upstream::RouteBudget {
                runtime,
                provider: &provider_key,
                wait_for_cooldown: candidate_count == 1,
            },
        )
        .await
        {
            Ok(response) => response,
            Err(failure) => {
                if index + 1 < candidate_count && upstream::failover_eligible(failure.class) {
                    last_error = Some(failure.message);
                    continue;
                }
                return Err(ApiError::upstream(StatusCode::BAD_GATEWAY, failure.message));
            }
        };
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let class = upstream::classify_status(status);
        let body = response.text().await.unwrap_or_default();
        let message = format!("upstream returned {status}: {body}");
        if index + 1 < candidate_count && upstream::failover_eligible(class) {
            last_error = Some(message);
            continue;
        }
        return Err(ApiError::upstream(StatusCode::BAD_GATEWAY, message));
    }
    Err(ApiError::upstream(
        StatusCode::BAD_GATEWAY,
        last_error.unwrap_or_else(|| "all combo candidates failed".into()),
    ))
}

use anyhow::Context;
use async_stream::stream;
use axum::{
    Json, Router,
    body::{Body, Bytes as AxumBytes},
    extract::{DefaultBodyLimit, OriginalUri, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get, post},
};
use bytes::Bytes;
use futures_util::{Stream, TryStreamExt};
use serde_json::{Value, json};
use tokio::io::AsyncBufReadExt;
use tokio_util::io::StreamReader;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;

use crate::{
    antigravity, autostart,
    dashboard::{self, DashboardData},
    desktop::{self, DesktopTargets},
    error::ApiError,
    local_config, protocol,
    provider::{ModelInfo, Registry, RegistryStore},
    sources::SourceSelection,
    target_config::TargetPreferences,
    upgrade, upstream,
};

#[derive(Clone)]
struct AppState {
    registry: RegistryStore,
    stream_idle_timeout: Duration,
    retry_policy: upstream::RetryPolicy,
    upstream_runtime: upstream::Runtime,
}

const DEFAULT_MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone, Debug)]
struct ServerPolicy {
    auth_token: Option<String>,
    allowed_origins: Vec<HeaderValue>,
    max_request_bytes: usize,
    stream_idle_timeout: Duration,
    retry_policy: upstream::RetryPolicy,
    upstream_runtime: upstream::Runtime,
    remote: bool,
}

impl ServerPolicy {
    fn from_host(host: IpAddr) -> anyhow::Result<Self> {
        let remote = !host.is_loopback();
        let auth_token = std::env::var("JOOCODE_API_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        if remote && auth_token.is_none() {
            anyhow::bail!("binding to non-loopback address {host} requires JOOCODE_API_AUTH_TOKEN");
        }
        let allowed_origins = std::env::var("JOOCODE_ALLOWED_ORIGINS")
            .ok()
            .into_iter()
            .flat_map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .map(|origin| {
                HeaderValue::from_str(&origin)
                    .with_context(|| format!("invalid JOOCODE_ALLOWED_ORIGINS entry '{origin}'"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let max_request_bytes = std::env::var("JOOCODE_MAX_REQUEST_BYTES")
            .ok()
            .map(|value| {
                value.parse::<usize>().with_context(|| {
                    format!("JOOCODE_MAX_REQUEST_BYTES must be a positive integer, got '{value}'")
                })
            })
            .transpose()?
            .unwrap_or(DEFAULT_MAX_REQUEST_BYTES);
        if max_request_bytes == 0 {
            anyhow::bail!("JOOCODE_MAX_REQUEST_BYTES must be greater than zero");
        }
        let stream_idle_timeout = std::env::var("JOOCODE_STREAM_IDLE_TIMEOUT_SECONDS")
            .ok()
            .map(|value| {
                value.parse::<u64>().with_context(|| {
                    format!(
                        "JOOCODE_STREAM_IDLE_TIMEOUT_SECONDS must be a positive integer, got '{value}'"
                    )
                })
            })
            .transpose()?
            .map(Duration::from_secs)
            .unwrap_or(DEFAULT_STREAM_IDLE_TIMEOUT);
        if stream_idle_timeout.is_zero() {
            anyhow::bail!("JOOCODE_STREAM_IDLE_TIMEOUT_SECONDS must be greater than zero");
        }
        let retry_policy = upstream::RetryPolicy::from_env()?;
        let upstream_runtime = upstream::Runtime::from_env()?;
        Ok(Self {
            auth_token,
            allowed_origins,
            max_request_bytes,
            stream_idle_timeout,
            retry_policy,
            upstream_runtime,
            remote,
        })
    }

    fn cors_layer(&self) -> CorsLayer {
        let layer = CorsLayer::new()
            .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
            .allow_headers([
                header::AUTHORIZATION,
                header::CONTENT_TYPE,
                header::HeaderName::from_static("x-joocode-api-key"),
                header::HeaderName::from_static("x-api-key"),
                header::HeaderName::from_static("anthropic-version"),
            ]);
        if self.remote {
            if self.allowed_origins.is_empty() {
                layer
            } else {
                layer.allow_origin(AllowOrigin::list(self.allowed_origins.clone()))
            }
        } else {
            layer.allow_origin(AllowOrigin::mirror_request())
        }
    }
}

async fn require_remote_auth(
    State(policy): State<ServerPolicy>,
    mut request: Request,
    next: Next,
) -> Response {
    if !policy.remote {
        return next.run(request).await;
    }
    let expected = policy.auth_token.as_deref().unwrap_or_default();
    let authorization = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    let api_key = request
        .headers()
        .get("x-joocode-api-key")
        .and_then(|value| value.to_str().ok());
    if api_key.is_some_and(|value| secure_eq(value, expected)) {
        request.headers_mut().remove("x-joocode-api-key");
        return next.run(request).await;
    }
    if authorization.is_some_and(|value| secure_eq(value, expected)) {
        request.headers_mut().remove(header::AUTHORIZATION);
        return next.run(request).await;
    }
    ApiError {
        status: StatusCode::UNAUTHORIZED,
        kind: "authentication_error",
        message: "missing or invalid Joocode API token".into(),
    }
    .into_response()
}

fn secure_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or_default()
                ^ right.get(index).copied().unwrap_or_default(),
        );
    }
    difference == 0
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
    let response = send_routed(
        &registry,
        routable_model,
        &state.retry_policy,
        &state.upstream_runtime,
        None,
        |upstream_model| protocol::anthropic_to_chat_request(&request, upstream_model),
    )
    .await?;
    if request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Ok(ResponseBody::Stream(anthropic_stream_response(
            response.bytes_stream(),
            requested_model.to_owned(),
            state.stream_idle_timeout,
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

fn anthropic_stream_response<S>(
    upstream: S,
    requested_model: String,
    idle_timeout: Duration,
) -> Response
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
        let mut terminal = false;
        let mut stream_error = None::<String>;
        loop {
            let line = match tokio::time::timeout(idle_timeout, lines.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    stream_error = Some(format!("upstream stream failed: {error}"));
                    break;
                }
                Err(_) => {
                    stream_error = Some(format!(
                        "upstream stream was idle for {} seconds",
                        idle_timeout.as_secs()
                    ));
                    break;
                }
            };
            let Some(data) = line.strip_prefix("data:") else { continue; };
            let data = data.trim();
            if data == "[DONE]" {
                terminal = true;
                break;
            }
            let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue; };
            terminal |= chunk
                .pointer("/choices/0/finish_reason")
                .is_some_and(|reason| !reason.is_null());
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
        if !terminal {
            let message = stream_error.unwrap_or_else(|| "upstream stream ended before completion".into());
            yield Ok(Bytes::from(format!(
                "event: error\ndata: {}\n\n",
                json!({"type":"error","error":{"type":"api_error","message":message}})
            )));
            return;
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
        let run_in_background = TargetPreferences::load()
            .map(|preferences| preferences.run_in_background)
            .unwrap_or(true);
        if self.active && run_in_background {
            let _ = autostart::resume_detached();
        } else if self.active {
            let _ = autostart::stop();
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
    Json(request): Json<Value>,
) -> Result<ResponseBody, ApiError> {
    let registry = state.registry.snapshot();
    let requested_model = request
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("missing 'model'"))?
        .to_owned();
    let local_key = headers.get("x-joocode-api-key").cloned();
    let response = send_routed(
        &registry,
        &requested_model,
        &state.retry_policy,
        &state.upstream_runtime,
        local_key,
        |upstream_model| {
            let mut request = request.clone();
            request["model"] = Value::String(upstream_model.to_owned());
            Ok(request)
        },
    )
    .await?;
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
    let response = upstream::send_json(
        registry.client(),
        url,
        &headers,
        &request,
        &state.retry_policy,
        upstream::RouteBudget {
            runtime: &state.upstream_runtime,
            provider: "openai-native",
            wait_for_cooldown: true,
        },
    )
    .await
    .map_err(|failure| ApiError::upstream(StatusCode::BAD_GATEWAY, failure.message))?;
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
    } = prepare_server(host, port, RegistryStore::new(registry), false).await?
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
    } = prepare_server(host, port, registry_store.clone(), interactive).await?
    else {
        persistent_proxy.disarm();
        println!("Joocode is already running in the background at http://{host}:{port}.");
        return Ok(());
    };
    let base_url = base_url.unwrap_or_else(|| desktop_base_url(address));
    let dashboard_data = DashboardData::new(&registry, &targets, &selection, address, port_warning);
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
        let mut active_selection = selection;
        while let Some(command) = command_rx.recv().await {
            match command {
                dashboard::DashboardCommand::AddProvider { base_url, api_key } => {
                    let result = async {
                        let client = reload_store.snapshot().client().clone();
                        let provider = local_config::probe(&client, &base_url, &api_key).await?;
                        local_config::save(provider.clone())?;
                        let registry = Registry::discover(&active_selection).await?;
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
                dashboard::DashboardCommand::ToggleRunInBackground => {
                    let enabled = !TargetPreferences::load()
                        .unwrap_or_default()
                        .run_in_background;
                    let event = match TargetPreferences::set_run_in_background(enabled) {
                        Ok(_) => dashboard::DashboardEvent::RunInBackgroundUpdated(enabled),
                        Err(error) => dashboard::DashboardEvent::ProviderError(error.to_string()),
                    };
                    let _ = event_tx.send(event);
                }
                dashboard::DashboardCommand::RemoveProvider { name } => {
                    let result = async {
                        local_config::remove(&name)?;
                        let registry = Registry::discover(&active_selection).await?;
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
                dashboard::DashboardCommand::SetDefaultModel { provider, model } => {
                    let result = (|| {
                        local_config::set_default_model(&provider, &model)?;
                        let registry = reload_store.snapshot();
                        let setup_targets = active_targets.clone();
                        desktop::configure_detected(&registry, &reload_base_url, &setup_targets);
                        local_config::summaries()
                    })();
                    let event = match result {
                        Ok(providers) => dashboard::DashboardEvent::ProviderDefaultUpdated {
                            provider,
                            providers,
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
                dashboard::DashboardCommand::ToggleSource { source } => {
                    let enabled = !active_selection.enabled(source);
                    let result = async {
                        let mut selection = active_selection.clone();
                        selection.set_enabled(source, enabled);
                        let registry = Registry::discover(&selection).await?;
                        TargetPreferences::set_source(source, enabled)?;
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
                        Ok::<_, anyhow::Error>((selection, registry))
                    }
                    .await;
                    let event = match result {
                        Ok((selection, registry)) => {
                            active_selection = selection;
                            dashboard::DashboardEvent::SourceUpdated {
                                source,
                                enabled,
                                config_sources: dashboard::config_sources(&registry),
                                model_count: registry.models().len(),
                                provider_count: registry.provider_count(),
                            }
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
                    let event = match upgrade::install_for_restart(&tag).await {
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
            let run_in_background = TargetPreferences::load()
                .unwrap_or_default()
                .run_in_background;
            if run_in_background {
                autostart::resume_detached()?;
            } else {
                autostart::stop()?;
            }
            persistent_proxy.disarm();
            if run_in_background {
                println!(
                    "Joocode is still running in the background. You can stop it with `jcx stop`."
                );
            }
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
    reclaim_requested_port: bool,
) -> anyhow::Result<PreparedServer> {
    let policy = ServerPolicy::from_host(host)?;
    let app = build_router(registry, &policy);
    match bind_available(host, port, reclaim_requested_port).await? {
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

fn build_router(registry: RegistryStore, policy: &ServerPolicy) -> Router {
    let state = AppState {
        registry,
        stream_idle_timeout: policy.stream_idle_timeout,
        retry_policy: policy.retry_policy.clone(),
        upstream_runtime: policy.upstream_runtime.clone(),
    };
    let protected = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .route("/v1/chat/completions", post(chat_completions))
        .route("/v1/messages", post(anthropic_messages))
        .route("/v1/messages/count_tokens", post(anthropic_count_tokens))
        .route("/v1internal:fetchAvailableModels", post(antigravity_bridge))
        .route("/v1internal:generateContent", post(antigravity_bridge))
        .route(
            "/v1internal:streamGenerateContent",
            post(antigravity_bridge),
        )
        .route("/v1internal:{*path}", any(antigravity_bridge))
        .route("/v1beta/models", get(antigravity_bridge))
        .route("/v1beta/{*path}", any(antigravity_bridge))
        .route_layer(middleware::from_fn_with_state(
            policy.clone(),
            require_remote_auth,
        ));
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/hello", get(healthz))
        .route("/readyz", get(readyz))
        .merge(protected)
        .with_state(state)
        .layer(DefaultBodyLimit::max(policy.max_request_bytes))
        .layer(policy.cors_layer())
        .layer(TraceLayer::new_for_http())
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

async fn bind_available(
    host: IpAddr,
    requested_port: u16,
    reclaim_requested_port: bool,
) -> std::io::Result<BindResult> {
    let requested_address = SocketAddr::from((host, requested_port));
    match tokio::net::TcpListener::bind(requested_address).await {
        Ok(listener) => Ok(BindResult::Bound {
            listener,
            port_warning: None,
        }),
        Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
            // The interactive dashboard pauses the persistent Joocode daemon
            // immediately before binding. Service managers can finish the stop
            // operation a few milliseconds before the process releases its
            // socket, so briefly wait for our original port before falling back.
            if reclaim_requested_port {
                for _ in 0..20 {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    match tokio::net::TcpListener::bind(requested_address).await {
                        Ok(listener) => {
                            return Ok(BindResult::Bound {
                                listener,
                                port_warning: None,
                            });
                        }
                        Err(retry_error) if retry_error.kind() == std::io::ErrorKind::AddrInUse => {
                        }
                        Err(retry_error) => return Err(retry_error),
                    }
                }
            }
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

async fn readyz(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.snapshot();
    let models = registry.models().len();
    let providers = registry.provider_count();
    let source_errors = registry
        .source_reports()
        .iter()
        .filter(|report| report.status == "error")
        .count();
    let (status, readiness) = if models == 0 || providers == 0 {
        (StatusCode::SERVICE_UNAVAILABLE, "failed")
    } else if source_errors > 0 {
        (StatusCode::OK, "degraded")
    } else {
        (StatusCode::OK, "ready")
    };
    (
        status,
        [
            ("x-joocode-service", "joocode"),
            ("x-joocode-readiness", readiness),
        ],
        Json(json!({
            "ready": status.is_success(),
            "status": readiness,
            "service": "joocode",
            "version": env!("CARGO_PKG_VERSION"),
            "providers": providers,
            "models": models,
            "source_errors": source_errors
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
    let first_candidate = registry
        .resolve_candidates(&requested_model)
        .map_err(|error| ApiError::not_found(error.to_string()))?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::not_found(format!("unknown model '{requested_model}'")))?;
    let chat_request = protocol::to_chat_request(&request, &first_candidate.2)?;
    let tool_namespaces = chat_request.tool_namespaces.clone();
    let local_key = headers
        .get("x-joocode-api-key")
        .or_else(|| headers.get("x-joc-api-key"))
        .or_else(|| headers.get("x-open-initiative-api-key"))
        .cloned();
    let response = send_routed(
        &registry,
        &requested_model,
        &state.retry_policy,
        &state.upstream_runtime,
        local_key,
        |upstream_model| {
            protocol::to_chat_request(&request, upstream_model).map(|request| request.body)
        },
    )
    .await?;
    if request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Ok(ResponseBody::Stream(stream_response(
            response.bytes_stream(),
            requested_model,
            tool_namespaces,
            state.stream_idle_timeout,
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
            &tool_namespaces,
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

fn stream_response<S>(
    upstream: S,
    requested_model: String,
    tool_namespaces: protocol::ToolNamespaces,
    idle_timeout: Duration,
) -> Response
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let events = stream! {
        let mut state = protocol::StreamState::new(protocol::response_id(), requested_model, tool_namespaces);
        for frame in state.created_events() { yield Ok::<Bytes, Infallible>(Bytes::from(frame)); }
        let reader = StreamReader::new(upstream.map_err(std::io::Error::other));
        let mut lines = reader.lines();
        let mut terminal = false;
        let mut incomplete_reason = None::<String>;
        loop {
            let line = match tokio::time::timeout(idle_timeout, lines.next_line()).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    incomplete_reason = Some(format!("upstream stream failed: {error}"));
                    break;
                }
                Err(_) => {
                    incomplete_reason = Some(format!(
                        "upstream stream was idle for {} seconds",
                        idle_timeout.as_secs()
                    ));
                    break;
                }
            };
            let Some(data) = line.strip_prefix("data:") else { continue; };
            let data = data.trim();
            if data == "[DONE]" {
                terminal = true;
                break;
            }
            let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue; };
            terminal |= chunk
                .pointer("/choices/0/finish_reason")
                .is_some_and(|reason| !reason.is_null());
            for frame in state.consume_chunk(&chunk) { yield Ok(Bytes::from(frame)); }
        }
        let final_events = if terminal {
            state.completed_events()
        } else {
            state.incomplete_events(
                incomplete_reason.as_deref().unwrap_or("upstream stream ended before completion")
            )
        };
        for frame in final_events { yield Ok(Bytes::from(frame)); }
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
    use axum::body::to_bytes;
    use http::Request as HttpRequest;
    use tower::ServiceExt;

    fn fixture_registry() -> Registry {
        use crate::sources::{DiscoveredCatalog, DiscoveredModel, DiscoveredProvider};
        use reqwest::{Client, header::HeaderMap};

        let model = ModelInfo {
            id: "fixture/model-a".into(),
            provider: "fixture".into(),
            upstream_id: "model-a".into(),
            name: "Model A".into(),
            reasoning: false,
            context_window: Some(128_000),
            max_output_tokens: Some(8_192),
        };
        Registry::from_catalogs_for_test(
            Client::new(),
            vec![Ok(DiscoveredCatalog {
                source: "fixture".into(),
                detail: None,
                providers: vec![DiscoveredProvider {
                    key: "fixture".into(),
                    provider: crate::provider::Provider {
                        base_url: "https://example.test/v1".into(),
                        credential: crate::provider::Credential::None,
                        headers: HeaderMap::new(),
                    },
                    models: vec![DiscoveredModel { info: model }],
                }],
            })],
        )
        .unwrap()
    }

    fn policy(remote: bool, token: Option<&str>, origins: &[&str], limit: usize) -> ServerPolicy {
        ServerPolicy {
            auth_token: token.map(str::to_owned),
            allowed_origins: origins
                .iter()
                .map(|origin| HeaderValue::from_str(origin).unwrap())
                .collect(),
            max_request_bytes: limit,
            stream_idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
            retry_policy: upstream::RetryPolicy::default(),
            upstream_runtime: upstream::Runtime::new(8, Duration::ZERO),
            remote,
        }
    }

    #[tokio::test]
    async fn health_route_is_available() {
        assert_eq!(healthz().await.into_response().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn readiness_reports_loaded_registry() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(false, None, &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let response = app
            .oneshot(HttpRequest::get("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("x-joocode-readiness").unwrap(),
            "ready"
        );
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["models"], 1);
        assert_eq!(body["providers"], 1);
    }

    #[tokio::test]
    async fn remote_data_plane_requires_the_configured_token() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(true, Some("secret"), &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let unauthorized = app
            .clone()
            .oneshot(HttpRequest::get("/v1/models").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);

        let authorized = app
            .oneshot(
                HttpRequest::get("/v1/models")
                    .header(header::AUTHORIZATION, "Bearer secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(authorized.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn remote_admission_token_is_removed_before_the_handler() {
        async fn echo_headers(headers: HeaderMap) -> Json<Value> {
            Json(json!({
                "authorization": headers.contains_key(header::AUTHORIZATION),
                "joocode_key": headers.contains_key("x-joocode-api-key")
            }))
        }
        let remote = policy(true, Some("secret"), &[], DEFAULT_MAX_REQUEST_BYTES);
        let app = Router::new()
            .route("/protected", get(echo_headers))
            .route_layer(middleware::from_fn_with_state(
                remote.clone(),
                require_remote_auth,
            ));

        for (name, value) in [
            (header::AUTHORIZATION, "Bearer secret"),
            (
                header::HeaderName::from_static("x-joocode-api-key"),
                "secret",
            ),
        ] {
            let response = app
                .clone()
                .oneshot(
                    HttpRequest::get("/protected")
                        .header(name, value)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body, json!({"authorization":false,"joocode_key":false}));
        }
    }

    #[tokio::test]
    async fn readiness_fails_when_the_registry_has_no_routes() {
        use reqwest::Client;

        let registry = Registry::from_catalogs_for_test(Client::new(), vec![]).unwrap();
        let app = build_router(
            RegistryStore::new(registry),
            &policy(false, None, &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let response = app
            .oneshot(HttpRequest::get("/readyz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response.headers().get("x-joocode-readiness").unwrap(),
            "failed"
        );
    }

    #[tokio::test]
    async fn loopback_data_plane_remains_zero_configuration() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(false, None, &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let response = app
            .oneshot(HttpRequest::get("/v1/models").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn remote_cors_only_allows_configured_origins() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(
                true,
                Some("secret"),
                &["https://allowed.example"],
                DEFAULT_MAX_REQUEST_BYTES,
            ),
        );
        let allowed = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .method(Method::OPTIONS)
                    .uri("/v1/models")
                    .header(header::ORIGIN, "https://allowed.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            allowed
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "https://allowed.example"
        );
        let denied = app
            .oneshot(
                HttpRequest::builder()
                    .method(Method::OPTIONS)
                    .uri("/v1/models")
                    .header(header::ORIGIN, "https://denied.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            denied
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[tokio::test]
    async fn request_bodies_are_bounded() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(false, None, &[], 32),
        );
        let response = app
            .oneshot(
                HttpRequest::post("/v1/responses")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(vec![b'x'; 64]))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn stalled_responses_stream_is_reported_as_incomplete() {
        let upstream = futures_util::stream::pending::<Result<Bytes, reqwest::Error>>();
        let response = stream_response(
            upstream,
            "fixture/model-a".into(),
            protocol::ToolNamespaces::default(),
            Duration::from_millis(1),
        );
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: response.incomplete"));
        assert!(body.contains("upstream_stream_interrupted"));
        assert!(!body.contains("event: response.completed"));
    }

    #[tokio::test]
    async fn stalled_anthropic_stream_emits_an_error_event() {
        let upstream = futures_util::stream::pending::<Result<Bytes, reqwest::Error>>();
        let response =
            anthropic_stream_response(upstream, "fixture/model-a".into(), Duration::from_millis(1));
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: error"));
        assert!(body.contains("upstream stream was idle"));
        assert!(!body.contains("event: message_stop"));
    }

    #[tokio::test]
    async fn combo_fails_over_to_the_next_candidate() {
        use crate::{
            combo::Combo,
            provider::{Credential, Provider},
            sources::{DiscoveredCatalog, DiscoveredModel, DiscoveredProvider},
        };
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        async fn provider_server(status: StatusCode, attempts: Arc<AtomicUsize>) -> SocketAddr {
            let app = Router::new().route(
                "/v1/chat/completions",
                post(move |Json(body): Json<Value>| {
                    let attempts = attempts.clone();
                    async move {
                        attempts.fetch_add(1, Ordering::SeqCst);
                        (
                            status,
                            Json(json!({
                                "model": body["model"],
                                "choices": [{"message":{"role":"assistant","content":"ok"}}]
                            })),
                        )
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            address
        }

        let first_attempts = Arc::new(AtomicUsize::new(0));
        let second_attempts = Arc::new(AtomicUsize::new(0));
        let first = provider_server(StatusCode::SERVICE_UNAVAILABLE, first_attempts.clone()).await;
        let second = provider_server(StatusCode::OK, second_attempts.clone()).await;
        let make_provider = |key: &str, address: SocketAddr, model: &str| DiscoveredProvider {
            key: key.into(),
            provider: Provider {
                base_url: format!("http://{address}/v1"),
                credential: Credential::None,
                headers: HeaderMap::new(),
            },
            models: vec![DiscoveredModel {
                info: ModelInfo {
                    id: format!("{key}/{model}"),
                    provider: key.into(),
                    upstream_id: model.into(),
                    name: model.into(),
                    reasoning: false,
                    context_window: None,
                    max_output_tokens: None,
                },
            }],
        };
        let registry = Registry::from_catalogs_and_combos_for_test(
            reqwest::Client::new(),
            vec![Ok(DiscoveredCatalog {
                source: "fixture".into(),
                detail: None,
                providers: vec![
                    make_provider("first", first, "model-a"),
                    make_provider("second", second, "model-b"),
                ],
            })],
            vec![Combo {
                name: "coding".into(),
                models: vec!["first/model-a".into(), "second/model-b".into()],
            }],
        )
        .unwrap();

        let response = send_routed(
            &registry,
            "combo/coding",
            &upstream::RetryPolicy {
                max_attempts: 1,
                initial_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            },
            &upstream::Runtime::new(8, Duration::ZERO),
            None,
            |model| Ok(json!({"model": model})),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(first_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(second_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(response.json::<Value>().await.unwrap()["model"], "model-b");
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
        } = bind_available("127.0.0.1".parse().unwrap(), requested_port, false)
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

        let result = bind_available("127.0.0.1".parse().unwrap(), port, false)
            .await
            .unwrap();

        assert!(matches!(result, BindResult::ExistingJoocode));
        server.abort();
    }

    #[tokio::test]
    async fn dashboard_reclaims_port_after_the_background_daemon_releases_it() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let requested_port = occupied.local_addr().unwrap().port();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(75)).await;
            drop(occupied);
        });

        let BindResult::Bound {
            listener,
            port_warning,
        } = bind_available("127.0.0.1".parse().unwrap(), requested_port, true)
            .await
            .unwrap()
        else {
            panic!("the dashboard should reclaim the daemon's original port");
        };

        assert_eq!(listener.local_addr().unwrap().port(), requested_port);
        assert!(port_warning.is_none());
    }
}
