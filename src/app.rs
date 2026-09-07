use std::{
    collections::BTreeMap,
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
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

async fn remote_rate_limit(
    State(limiter): State<RateLimiter>,
    request: Request,
    next: Next,
) -> Response {
    if limiter.allow().await {
        next.run(request).await
    } else {
        (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "1")],
            Json(json!({
                "error": {
                    "type": "rate_limit_error",
                    "message": "Joocode remote request rate limit exceeded"
                }
            })),
        )
            .into_response()
    }
}

fn prometheus_label(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

async fn api_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.snapshot();
    let provider_statuses = state
        .upstream_runtime
        .provider_statuses(&registry.provider_keys())
        .await;
    let mut output = String::new();
    macro_rules! metric {
        ($name:literal, $help:literal, $kind:literal, $value:expr) => {
            output.push_str(concat!("# HELP ", $name, " ", $help, "\n"));
            output.push_str(concat!("# TYPE ", $name, " ", $kind, "\n"));
            output.push_str(&format!(concat!($name, " {}\n"), $value));
        };
    }
    metric!(
        "joocode_uptime_seconds",
        "Seconds since the Joocode process started.",
        "gauge",
        state.metrics.started.elapsed().as_secs()
    );
    metric!(
        "joocode_providers",
        "Number of discovered logical providers.",
        "gauge",
        registry.provider_count()
    );
    metric!(
        "joocode_models",
        "Number of discovered routable models.",
        "gauge",
        registry.models().len()
    );
    metric!(
        "joocode_requests_total",
        "Total protected data-plane and management requests.",
        "counter",
        state.metrics.requests.load(Ordering::Relaxed)
    );
    metric!(
        "joocode_active_requests",
        "Requests currently executing.",
        "gauge",
        state.metrics.active.load(Ordering::Relaxed)
    );
    metric!(
        "joocode_responses_success_total",
        "Successful protected responses.",
        "counter",
        state.metrics.successes.load(Ordering::Relaxed)
    );
    metric!(
        "joocode_responses_failure_total",
        "Failed protected responses.",
        "counter",
        state.metrics.failures.load(Ordering::Relaxed)
    );
    metric!(
        "joocode_tool_calls_total",
        "Tool calls returned to downstream clients.",
        "counter",
        state.metrics.tool_calls.load(Ordering::Relaxed)
    );
    metric!(
        "joocode_codex_browser_tool_calls_total",
        "Codex browser or computer-control tool calls returned to clients.",
        "counter",
        state.metrics.browser_tool_calls.load(Ordering::Relaxed)
    );
    output.push_str(
        "# HELP joocode_tool_call_total Tool calls grouped by namespace and tool name.\n\
# TYPE joocode_tool_call_total counter\n",
    );
    for (tool, count) in state
        .metrics
        .tool_call_breakdown
        .lock()
        .expect("tool-call metrics lock poisoned")
        .iter()
    {
        let (namespace, name) = tool.split_once('/').unwrap_or(("function", tool));
        output.push_str(&format!(
            "joocode_tool_call_total{{namespace=\"{}\",tool=\"{}\"}} {count}\n",
            prometheus_label(namespace),
            prometheus_label(name)
        ));
    }
    output.push_str(
        "# HELP joocode_provider_active_requests Active requests for a provider route.\n\
# TYPE joocode_provider_active_requests gauge\n\
# HELP joocode_provider_concurrency_limit Configured concurrency limit for a provider route.\n\
# TYPE joocode_provider_concurrency_limit gauge\n\
# HELP joocode_provider_cooldown_seconds Remaining provider cooldown in seconds.\n\
# TYPE joocode_provider_cooldown_seconds gauge\n\
# HELP joocode_provider_available Whether a provider route is currently available.\n\
# TYPE joocode_provider_available gauge\n",
    );
    for provider in provider_statuses {
        let name = prometheus_label(&provider.provider);
        output.push_str(&format!(
            "joocode_provider_active_requests{{provider=\"{name}\"}} {}\n",
            provider.active_requests
        ));
        output.push_str(&format!(
            "joocode_provider_concurrency_limit{{provider=\"{name}\"}} {}\n",
            provider.concurrency_limit
        ));
        output.push_str(&format!(
            "joocode_provider_cooldown_seconds{{provider=\"{name}\"}} {:.3}\n",
            provider.cooldown_ms as f64 / 1000.0
        ));
        output.push_str(&format!(
            "joocode_provider_available{{provider=\"{name}\"}} {}\n",
            u8::from(provider.state == "available")
        ));
    }
    (
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        output,
    )
}

pub async fn reload(url: &str, token: Option<&str>) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let mut request = client.post(url);
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        request = request.header("x-joocode-api-key", token);
    }
    let response = request.send().await?.error_for_status()?;
    let result = response.json::<Value>().await?;
    println!(
        "Joocode reloaded: {} providers, {} models",
        result["providers"], result["models"]
    );
    Ok(())
}

async fn api_reload(State(state): State<AppState>) -> Result<impl IntoResponse, ApiError> {
    let selection = state.source_selection.clone().ok_or_else(|| ApiError {
        status: StatusCode::CONFLICT,
        kind: "reload_unavailable",
        message: "provider reload is unavailable for this server instance".into(),
    })?;
    let registry = Registry::discover(&selection)
        .await
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    if registry.models().is_empty() || registry.provider_count() == 0 {
        return Err(ApiError {
            status: StatusCode::SERVICE_UNAVAILABLE,
            kind: "reload_failed",
            message: "provider reload produced an empty registry; the existing registry was kept"
                .into(),
        });
    }
    let providers = registry.provider_count();
    let models = registry.models().len();
    state.registry.replace(registry);
    Ok(Json(json!({
        "status": "reloaded",
        "providers": providers,
        "models": models,
    })))
}

async fn api_status(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.snapshot();
    let provider_statuses = state
        .upstream_runtime
        .provider_statuses(&registry.provider_keys())
        .await;
    Json(json!({
        "service": "joocode",
        "version": env!("CARGO_PKG_VERSION"),
        "uptime_seconds": state.metrics.started.elapsed().as_secs(),
        "providers": registry.provider_count(),
        "models": registry.models().len(),
        "requests": state.metrics.requests.load(Ordering::Relaxed),
        "active_requests": state.metrics.active.load(Ordering::Relaxed),
        "successful_responses": state.metrics.successes.load(Ordering::Relaxed),
        "failed_responses": state.metrics.failures.load(Ordering::Relaxed),
        "tool_calls": state.metrics.tool_calls.load(Ordering::Relaxed),
        "codex_browser_tool_calls": state.metrics.browser_tool_calls.load(Ordering::Relaxed),
        "provider_statuses": provider_statuses,
    }))
}

async fn api_providers(State(state): State<AppState>) -> impl IntoResponse {
    let registry = state.registry.snapshot();
    let mut grouped = std::collections::BTreeMap::<String, Vec<&crate::provider::ModelInfo>>::new();
    for model in registry.models() {
        grouped
            .entry(model.provider.clone())
            .or_default()
            .push(model);
    }
    let providers = grouped
        .into_iter()
        .map(|(provider, models)| {
            json!({
                "provider": provider,
                "model_count": models.len(),
                "models": models,
            })
        })
        .collect::<Vec<_>>();
    let sources = registry
        .source_reports()
        .iter()
        .map(|report| {
            json!({
                "source": report.source,
                "status": report.status,
                "providers": report.providers,
                "models": report.models,
                "detail": report.detail,
            })
        })
        .collect::<Vec<_>>();
    let runtime = state
        .upstream_runtime
        .provider_statuses(&registry.provider_keys())
        .await;
    Json(json!({
        "providers": providers,
        "sources": sources,
        "runtime": runtime,
    }))
}

pub async fn stats(url: &str, token: Option<&str>) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let mut request = client.get(url);
    if let Some(token) = token.filter(|token| !token.is_empty()) {
        request = request.header("x-joocode-api-key", token);
    }
    let response = request.send().await?.error_for_status()?;
    let status = response.json::<Value>().await?;
    println!(
        "Joocode {}",
        status["version"].as_str().unwrap_or("unknown")
    );
    println!("uptime:    {}s", status["uptime_seconds"]);
    println!("providers: {}", status["providers"]);
    println!("models:    {}", status["models"]);
    println!("requests:  {}", status["requests"]);
    println!("active:    {}", status["active_requests"]);
    println!("successes: {}", status["successful_responses"]);
    println!("failures:  {}", status["failed_responses"]);
    println!("tool calls: {}", status["tool_calls"]);
    println!("browser:    {}", status["codex_browser_tool_calls"]);
    if let Some(providers) = status["provider_statuses"].as_array()
        && !providers.is_empty()
    {
        println!("\nprovider runtime:");
        for provider in providers {
            println!(
                "  {:<24} {:<12} active={}/{} cooldown={}ms",
                provider["provider"].as_str().unwrap_or("unknown"),
                provider["state"].as_str().unwrap_or("unknown"),
                provider["active_requests"],
                provider["concurrency_limit"],
                provider["cooldown_ms"],
            );
        }
    }

    Ok(())
}

async fn next_bounded_sse_line<S>(
    upstream: &mut S,
    buffer: &mut Vec<u8>,
    max_bytes: usize,
) -> Result<Option<String>, String>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    loop {
        if let Some(newline) = buffer.iter().position(|byte| *byte == b'\n') {
            let mut line = buffer.drain(..=newline).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return String::from_utf8(line)
                .map(Some)
                .map_err(|error| format!("upstream SSE contained invalid UTF-8: {error}"));
        }
        if buffer.len() > max_bytes {
            return Err(format!(
                "upstream SSE event exceeded the {max_bytes} byte limit"
            ));
        }
        match upstream.next().await {
            Some(Ok(chunk)) => {
                if buffer.len().saturating_add(chunk.len()) > max_bytes {
                    return Err(format!(
                        "upstream SSE event exceeded the {max_bytes} byte limit"
                    ));
                }
                buffer.extend_from_slice(&chunk);
            }
            Some(Err(error)) => return Err(format!("upstream stream failed: {error}")),
            None if buffer.is_empty() => return Ok(None),
            None => {
                let line = std::mem::take(buffer);
                return String::from_utf8(line)
                    .map(Some)
                    .map_err(|error| format!("upstream SSE contained invalid UTF-8: {error}"));
            }
        }
    }
}

fn positive_usize_env(name: &str, default: usize) -> anyhow::Result<usize> {
    let value = std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("{name} must be a positive integer, got '{value}'"))
        })
        .transpose()?
        .unwrap_or(default);
    if value == 0 {
        anyhow::bail!("{name} must be greater than zero");
    }
    Ok(value)
}

fn compact_upstream_url(chatgpt_session: bool) -> &'static str {
    if chatgpt_session {
        "https://chatgpt.com/backend-api/codex/responses/compact"
    } else {
        "https://api.openai.com/v1/responses/compact"
    }
}

async fn compact_responses(
    State(state): State<AppState>,
    mut headers: HeaderMap,
    Json(request): Json<Value>,
) -> Result<ResponseBody, ApiError> {
    let model = request
        .get("model")
        .and_then(Value::as_str)
        .ok_or_else(|| ApiError::bad_request("missing 'model'"))?;
    if model.contains('/') {
        return Err(ApiError {
            status: StatusCode::NOT_IMPLEMENTED,
            kind: "unsupported_feature",
            message: "routed models do not expose native Responses compaction; use an OpenAI native model or shorten the conversation client-side".into(),
        });
    }
    let registry = state.registry.snapshot();
    let url = compact_upstream_url(headers.contains_key("chatgpt-account-id"));
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
    let body = response
        .json::<Value>()
        .await
        .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
    if !status.is_success() {
        return Err(ApiError::upstream(
            StatusCode::BAD_GATEWAY,
            format!("upstream returned {status}: {body}"),
        ));
    }
    Ok(ResponseBody::Json(Json(body)))
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
use futures_util::{Stream, StreamExt, TryStreamExt};
use serde_json::{Value, json};
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
    source_selection: Option<SourceSelection>,
    stream_idle_timeout: Duration,
    max_sse_event_bytes: usize,
    max_tool_argument_bytes: usize,
    metrics: Metrics,
    retry_policy: upstream::RetryPolicy,
    upstream_runtime: upstream::Runtime,
}

#[derive(Clone, Debug)]
struct Metrics {
    started: std::time::Instant,
    requests: Arc<AtomicU64>,
    active: Arc<AtomicU64>,
    successes: Arc<AtomicU64>,
    failures: Arc<AtomicU64>,
    tool_calls: Arc<AtomicU64>,
    browser_tool_calls: Arc<AtomicU64>,
    tool_call_breakdown: Arc<std::sync::Mutex<BTreeMap<String, u64>>>,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            started: std::time::Instant::now(),
            requests: Arc::new(AtomicU64::new(0)),
            active: Arc::new(AtomicU64::new(0)),
            successes: Arc::new(AtomicU64::new(0)),
            failures: Arc::new(AtomicU64::new(0)),
            tool_calls: Arc::new(AtomicU64::new(0)),
            browser_tool_calls: Arc::new(AtomicU64::new(0)),
            tool_call_breakdown: Arc::new(std::sync::Mutex::new(BTreeMap::new())),
        }
    }
}

impl Metrics {
    fn record_tool_call(&self, namespace: Option<&str>, name: &str) {
        self.tool_calls.fetch_add(1, Ordering::Relaxed);
        let normalized_namespace = namespace.unwrap_or("function");
        if normalized_namespace.to_ascii_lowercase().contains("browser")
            || normalized_namespace.to_ascii_lowercase().contains("computer")
            || name.to_ascii_lowercase().contains("browser")
            || name.to_ascii_lowercase().contains("computer")
        {
            self.browser_tool_calls.fetch_add(1, Ordering::Relaxed);
        }
        let key = format!("{normalized_namespace}/{name}");
        *self
            .tool_call_breakdown
            .lock()
            .expect("tool-call metrics lock poisoned")
            .entry(key)
            .or_default() += 1;
    }

    fn record_responses_tool_calls(&self, response: &Value) {
        let Some(output) = response.get("output").and_then(Value::as_array) else {
            return;
        };
        for item in output {
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                continue;
            }
            let namespace = item.get("namespace").and_then(Value::as_str);
            if let Some(name) = item.get("name").and_then(Value::as_str) {
                self.record_tool_call(namespace, name);
            }
        }
    }
}

struct ActiveRequest {
    metrics: Metrics,
}

impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.metrics.active.fetch_sub(1, Ordering::Relaxed);
    }
}

async fn record_request(State(metrics): State<Metrics>, request: Request, next: Next) -> Response {
    metrics.requests.fetch_add(1, Ordering::Relaxed);
    metrics.active.fetch_add(1, Ordering::Relaxed);
    let _active = ActiveRequest {
        metrics: metrics.clone(),
    };
    let response = next.run(request).await;
    if response.status().is_success() {
        metrics.successes.fetch_add(1, Ordering::Relaxed);
    } else {
        metrics.failures.fetch_add(1, Ordering::Relaxed);
    }
    response
}

const DEFAULT_MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const DEFAULT_STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_MAX_SSE_EVENT_BYTES: usize = 1024 * 1024;
const DEFAULT_MAX_TOOL_ARGUMENT_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug)]
struct ServerPolicy {
    auth_token: Option<String>,
    management_token: Option<String>,
    allowed_origins: Vec<HeaderValue>,
    max_request_bytes: usize,
    stream_idle_timeout: Duration,
    max_sse_event_bytes: usize,
    max_tool_argument_bytes: usize,
    retry_policy: upstream::RetryPolicy,
    upstream_runtime: upstream::Runtime,
    remote: bool,
    rate_limiter: RateLimiter,
}

#[derive(Clone, Debug)]
struct AuthPolicy {
    remote: bool,
    token: Option<String>,
    label: &'static str,
}

#[derive(Clone, Debug)]
struct RateLimiter {
    inner: Arc<tokio::sync::Mutex<RateLimitState>>,
    rate_per_second: f64,
    burst: f64,
    enabled: bool,
}

#[derive(Debug)]
struct RateLimitState {
    tokens: f64,
    updated: Instant,
}

impl RateLimiter {
    fn new(enabled: bool, rate_per_second: usize, burst: usize) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(RateLimitState {
                tokens: burst as f64,
                updated: Instant::now(),
            })),
            rate_per_second: rate_per_second as f64,
            burst: burst as f64,
            enabled,
        }
    }

    async fn allow(&self) -> bool {
        if !self.enabled {
            return true;
        }
        let mut state = self.inner.lock().await;
        let now = Instant::now();
        let elapsed = now.duration_since(state.updated).as_secs_f64();
        state.tokens = (state.tokens + elapsed * self.rate_per_second).min(self.burst);
        state.updated = now;
        if state.tokens < 1.0 {
            return false;
        }
        state.tokens -= 1.0;
        true
    }
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
        let management_token = std::env::var("JOOCODE_MANAGEMENT_AUTH_TOKEN")
            .ok()
            .filter(|value| !value.trim().is_empty());
        if remote && management_token.is_none() {
            anyhow::bail!(
                "binding to non-loopback address {host} requires JOOCODE_MANAGEMENT_AUTH_TOKEN"
            );
        }
        if remote && management_token == auth_token {
            anyhow::bail!(
                "JOOCODE_MANAGEMENT_AUTH_TOKEN must differ from JOOCODE_API_AUTH_TOKEN"
            );
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
        let max_sse_event_bytes =
            positive_usize_env("JOOCODE_MAX_SSE_EVENT_BYTES", DEFAULT_MAX_SSE_EVENT_BYTES)?;
        let max_tool_argument_bytes = positive_usize_env(
            "JOOCODE_MAX_TOOL_ARGUMENT_BYTES",
            DEFAULT_MAX_TOOL_ARGUMENT_BYTES,
        )?;
        let retry_policy = upstream::RetryPolicy::from_env()?;
        let upstream_runtime = upstream::Runtime::from_env()?;
        let rate_per_second = positive_usize_env("JOOCODE_REMOTE_REQUESTS_PER_SECOND", 20)?;
        let rate_burst = positive_usize_env("JOOCODE_REMOTE_REQUEST_BURST", 40)?;
        Ok(Self {
            auth_token,
            management_token,
            allowed_origins,
            max_request_bytes,
            stream_idle_timeout,
            max_sse_event_bytes,
            max_tool_argument_bytes,
            retry_policy,
            upstream_runtime,
            remote,
            rate_limiter: RateLimiter::new(remote, rate_per_second, rate_burst),
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

async fn require_auth(
    State(policy): State<AuthPolicy>,
    mut request: Request,
    next: Next,
) -> Response {
    if !policy.remote {
        return next.run(request).await;
    }
    let expected = policy.token.as_deref().unwrap_or_default();
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
        message: format!("missing or invalid Joocode {} token", policy.label),
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
    let declared_tools = protocol::anthropic_declared_tools(&request);
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
            declared_tools,
            state.stream_idle_timeout,
            state.max_sse_event_bytes,
            state.max_tool_argument_bytes,
        )))
    } else {
        let chat = response
            .json::<Value>()
            .await
            .map_err(|error| ApiError::upstream(StatusCode::BAD_GATEWAY, error.to_string()))?;
        Ok(ResponseBody::Json(Json(
            protocol::chat_to_anthropic_response(chat, requested_model, &declared_tools)?,
        )))
    }
}

fn anthropic_stream_response<S>(
    upstream: S,
    requested_model: String,
    declared_tools: std::collections::BTreeSet<String>,
    idle_timeout: Duration,
    max_sse_event_bytes: usize,
    max_tool_argument_bytes: usize,
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
        let mut upstream = upstream;
        let mut buffer = Vec::new();
        let mut output_tokens = 0_u64;
        let mut text_open = true;
        let mut tools = BTreeMap::<usize, usize>::new();
        let mut tool_argument_bytes = BTreeMap::<usize, usize>::new();
        let mut next_block = 1_usize;
        let mut terminal = false;
        let mut stream_error = None::<String>;
        loop {
            let line = match tokio::time::timeout(
                idle_timeout,
                next_bounded_sse_line(&mut upstream, &mut buffer, max_sse_event_bytes),
            ).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    stream_error = Some(error);
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
                let name = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("tool");
                if !declared_tools.contains(name) {
                    stream_error = Some(format!("upstream model called undeclared tool '{name}'"));
                    terminal = false;
                    break;
                }
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
                    let accumulated = tool_argument_bytes.entry(upstream_index).or_default();
                    if accumulated.saturating_add(arguments.len()) > max_tool_argument_bytes {
                        stream_error = Some(format!(
                            "tool arguments exceeded the {max_tool_argument_bytes} byte limit"
                        ));
                        terminal = false;
                        break;
                    }
                    *accumulated = accumulated.saturating_add(arguments.len());
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
            if stream_error.is_some() {
                break;
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

pub async fn serve(
    host: IpAddr,
    port: u16,
    registry: Registry,
    selection: SourceSelection,
) -> anyhow::Result<()> {
    let PreparedServer::Ready {
        listener,
        app,
        address,
        port_warning,
    } = prepare_server(
        host,
        port,
        RegistryStore::new(registry),
        Some(selection),
        false,
    )
    .await?
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
    } = prepare_server(
        host,
        port,
        registry_store.clone(),
        Some(selection.clone()),
        interactive,
    )
    .await?
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
    source_selection: Option<SourceSelection>,
    reclaim_requested_port: bool,
) -> anyhow::Result<PreparedServer> {
    let policy = ServerPolicy::from_host(host)?;
    let app = build_router_with_selection(registry, &policy, source_selection);
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

#[cfg(test)]
fn build_router(registry: RegistryStore, policy: &ServerPolicy) -> Router {
    build_router_with_selection(registry, policy, None)
}

fn build_router_with_selection(
    registry: RegistryStore,
    policy: &ServerPolicy,
    source_selection: Option<SourceSelection>,
) -> Router {
    let metrics = Metrics::default();
    let state = AppState {
        registry,
        source_selection,
        stream_idle_timeout: policy.stream_idle_timeout,
        max_sse_event_bytes: policy.max_sse_event_bytes,
        max_tool_argument_bytes: policy.max_tool_argument_bytes,
        retry_policy: policy.retry_policy.clone(),
        upstream_runtime: policy.upstream_runtime.clone(),
        metrics: metrics.clone(),
    };
    let management = Router::new()
        .route("/api/status", get(api_status))
        .route("/api/providers", get(api_providers))
        .route("/api/metrics", get(api_metrics))
        .route("/api/reload", post(api_reload))
        .route_layer(middleware::from_fn_with_state(
            AuthPolicy {
                remote: policy.remote,
                token: policy.management_token.clone(),
                label: "management",
            },
            require_auth,
        ));
    let data_plane = Router::new()
        .route("/v1/models", get(models))
        .route("/v1/responses", post(responses))
        .route("/v1/responses/compact", post(compact_responses))
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
            AuthPolicy {
                remote: policy.remote,
                token: policy.auth_token.clone(),
                label: "API",
            },
            require_auth,
        ));
    let protected = Router::new()
        .merge(management)
        .merge(data_plane)
        .route_layer(middleware::from_fn_with_state(
            policy.rate_limiter.clone(),
            remote_rate_limit,
        ))
        .route_layer(middleware::from_fn_with_state(metrics, record_request));
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
            state.max_sse_event_bytes,
            state.max_tool_argument_bytes,
            state.metrics,
        )))
    } else {
        let chat: Value = response
            .json()
            .await
            .map_err(|e| ApiError::upstream(StatusCode::BAD_GATEWAY, e.to_string()))?;
        let response = protocol::from_chat_response(
            chat,
            &requested_model,
            protocol::response_id(),
            &tool_namespaces,
        )?;
        state.metrics.record_responses_tool_calls(&response);
        Ok(ResponseBody::Json(Json(response)))
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
    max_sse_event_bytes: usize,
    max_tool_argument_bytes: usize,
    metrics: Metrics,
) -> Response
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Send + Unpin + 'static,
{
    let events = stream! {
        let mut state = protocol::StreamState::new(protocol::response_id(), requested_model, tool_namespaces)
            .with_max_tool_argument_bytes(max_tool_argument_bytes);
        for frame in state.created_events() { yield Ok::<Bytes, Infallible>(Bytes::from(frame)); }
        let mut upstream = upstream;
        let mut buffer = Vec::new();
        let mut terminal = false;
        let mut incomplete_reason = None::<String>;
        loop {
            let line = match tokio::time::timeout(
                idle_timeout,
                next_bounded_sse_line(&mut upstream, &mut buffer, max_sse_event_bytes),
            ).await {
                Ok(Ok(Some(line))) => line,
                Ok(Ok(None)) => break,
                Ok(Err(error)) => {
                    incomplete_reason = Some(error);
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
            match state.completed_events() {
                Ok(events) => {
                    for (namespace, name) in state.completed_tool_calls() {
                        metrics.record_tool_call(namespace.as_deref(), &name);
                    }
                    events
                },
                Err(error) => state.incomplete_events(&error.message),
            }
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
            management_token: token.map(|token| format!("management-{token}")),
            allowed_origins: origins
                .iter()
                .map(|origin| HeaderValue::from_str(origin).unwrap())
                .collect(),
            max_request_bytes: limit,
            stream_idle_timeout: DEFAULT_STREAM_IDLE_TIMEOUT,
            max_sse_event_bytes: DEFAULT_MAX_SSE_EVENT_BYTES,
            max_tool_argument_bytes: DEFAULT_MAX_TOOL_ARGUMENT_BYTES,
            retry_policy: upstream::RetryPolicy::default(),
            upstream_runtime: upstream::Runtime::new(8, Duration::ZERO),
            remote,
            rate_limiter: RateLimiter::new(remote, 100, 100),
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
    async fn status_reports_privacy_safe_request_counters() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(false, None, &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let response = app
            .clone()
            .oneshot(HttpRequest::get("/v1/models").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let response = app
            .oneshot(HttpRequest::get("/api/status").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["service"], "joocode");
        assert_eq!(body["providers"], 1);
        assert_eq!(body["models"], 1);
        assert_eq!(body["requests"], 2);
        assert_eq!(body["active_requests"], 1);
        assert_eq!(body["successful_responses"], 1);
        assert_eq!(body["failed_responses"], 0);
        assert!(body["provider_statuses"].is_array());
        assert!(body.get("prompt").is_none());
        assert!(body.get("body").is_none());
    }

    #[tokio::test]
    async fn providers_api_exposes_catalog_without_connection_secrets() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(false, None, &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let response = app
            .oneshot(
                HttpRequest::get("/api/providers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16_384).await.unwrap();
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["providers"][0]["provider"], "fixture");
        assert_eq!(body["providers"][0]["models"][0]["id"], "fixture/model-a");
        assert_eq!(body["sources"][0]["source"], "fixture");
        assert!(body["runtime"].is_array());
        let rendered = serde_json::to_string(&body).unwrap();
        for secret_field in ["api_key", "authorization", "base_url", "headers"] {
            assert!(!rendered.contains(secret_field));
        }
    }

    #[tokio::test]
    async fn metrics_api_exposes_prometheus_counters_without_secrets() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(false, None, &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let response = app
            .oneshot(
                HttpRequest::get("/api/metrics")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE).unwrap(),
            "text/plain; version=0.0.4; charset=utf-8"
        );
        let body = to_bytes(response.into_body(), 32_768).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("joocode_uptime_seconds"));
        assert!(body.contains("joocode_models 1"));
        assert!(body.contains("joocode_provider_available{provider=\"fixture\"} 1"));
        for secret in [
            "api_key",
            "authorization",
            "base_url",
            "https://example.test",
        ] {
            assert!(!body.contains(secret));
        }
    }

    #[tokio::test]
    async fn reload_is_unavailable_without_source_selection() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(false, None, &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let response = app
            .oneshot(
                HttpRequest::post("/api/reload")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
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
        let remote = AuthPolicy {
            remote: true,
            token: Some("secret".into()),
            label: "API",
        };
        let app = Router::new()
            .route("/protected", get(echo_headers))
            .route_layer(middleware::from_fn_with_state(
                remote.clone(),
                require_auth,
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
    async fn routed_models_report_native_compaction_as_unsupported() {
        let app = build_router(
            RegistryStore::new(fixture_registry()),
            &policy(false, None, &[], DEFAULT_MAX_REQUEST_BYTES),
        );
        let response = app
            .oneshot(
                HttpRequest::post("/v1/responses/compact")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        json!({"model":"fixture/model-a","input":"history"}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);
        let body = to_bytes(response.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("native Responses compaction"));
    }

    #[test]
    fn compaction_uses_the_matching_openai_backend() {
        assert_eq!(
            compact_upstream_url(false),
            "https://api.openai.com/v1/responses/compact"
        );
        assert_eq!(
            compact_upstream_url(true),
            "https://chatgpt.com/backend-api/codex/responses/compact"
        );
    }

    #[tokio::test]
    async fn stalled_responses_stream_is_reported_as_incomplete() {
        let upstream = futures_util::stream::pending::<Result<Bytes, reqwest::Error>>();
        let response = stream_response(
            upstream,
            "fixture/model-a".into(),
            protocol::ToolNamespaces::default(),
            Duration::from_millis(1),
            DEFAULT_MAX_SSE_EVENT_BYTES,
            DEFAULT_MAX_TOOL_ARGUMENT_BYTES,
            Metrics::default(),
        );
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: response.incomplete"));
        assert!(body.contains("upstream_stream_interrupted"));
        assert!(!body.contains("event: response.completed"));
    }

    #[tokio::test]
    async fn oversized_sse_event_is_reported_as_incomplete() {
        let upstream = futures_util::stream::iter([Ok::<Bytes, reqwest::Error>(Bytes::from(
            "data: this-event-is-too-large",
        ))]);
        let response = stream_response(
            upstream,
            "fixture/model-a".into(),
            protocol::ToolNamespaces::default(),
            Duration::from_secs(1),
            8,
            DEFAULT_MAX_TOOL_ARGUMENT_BYTES,
            Metrics::default(),
        );
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: response.incomplete"));
        assert!(body.contains("SSE event exceeded the 8 byte limit"));
        assert!(!body.contains("event: response.completed"));
    }

    #[tokio::test]
    async fn dropping_client_body_cancels_upstream_stream() {
        struct DropSignal(std::sync::Arc<std::sync::atomic::AtomicBool>);

        impl Drop for DropSignal {
            fn drop(&mut self) {
                self.0.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let dropped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = DropSignal(dropped.clone());
        let upstream = async_stream::stream! {
            let _signal = signal;
            futures_util::future::pending::<()>().await;
            #[allow(unreachable_code)]
            yield Ok::<Bytes, reqwest::Error>(Bytes::new());
        };
        let response = stream_response(
            Box::pin(upstream),
            "fixture/model-a".into(),
            protocol::ToolNamespaces::default(),
            Duration::from_secs(60),
            DEFAULT_MAX_SSE_EVENT_BYTES,
            DEFAULT_MAX_TOOL_ARGUMENT_BYTES,
            Metrics::default(),
        );
        let mut body = response.into_body().into_data_stream();
        let poll = tokio::time::timeout(Duration::from_millis(10), body.next()).await;
        assert!(poll.is_ok());
        drop(body);
        tokio::task::yield_now().await;
        assert!(dropped.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn stalled_anthropic_stream_emits_an_error_event() {
        let upstream = futures_util::stream::pending::<Result<Bytes, reqwest::Error>>();
        let response = anthropic_stream_response(
            upstream,
            "fixture/model-a".into(),
            std::collections::BTreeSet::new(),
            Duration::from_millis(1),
            DEFAULT_MAX_SSE_EVENT_BYTES,
            DEFAULT_MAX_TOOL_ARGUMENT_BYTES,
        );
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: error"));
        assert!(body.contains("upstream stream was idle"));
        assert!(!body.contains("event: message_stop"));
    }

    #[tokio::test]
    async fn anthropic_stream_rejects_undeclared_tools() {
        let chunk = format!(
            "data: {}\n\ndata: [DONE]\n\n",
            json!({"choices":[{"delta":{"tool_calls":[{
                "index":0,"id":"call_1","function":{"name":"unknown","arguments":"{}"}
            }]}}]})
        );
        let upstream =
            futures_util::stream::iter([Ok::<Bytes, reqwest::Error>(Bytes::from(chunk))]);
        let response = anthropic_stream_response(
            upstream,
            "fixture/model-a".into(),
            std::collections::BTreeSet::from(["read".into()]),
            Duration::from_secs(1),
            DEFAULT_MAX_SSE_EVENT_BYTES,
            DEFAULT_MAX_TOOL_ARGUMENT_BYTES,
        );
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        assert!(body.contains("event: error"));
        assert!(body.contains("undeclared tool"));
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
                strategy: crate::combo::Strategy::Failover,
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
