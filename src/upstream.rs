use std::{collections::HashMap, pin::Pin, sync::Arc, time::Duration};

use anyhow::Context;
use bytes::Bytes;
use futures_util::{Stream, StreamExt};
use reqwest::{Client, Response, StatusCode, header::HeaderMap};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureClass {
    Authentication,
    RateLimited,
    Capacity,
    Timeout,
    Transport,
    Cooldown,
    InvalidRequest,
    Provider,
}

#[derive(Clone, Debug)]
pub struct RetryPolicy {
    pub max_attempts: usize,
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            initial_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(2),
        }
    }
}

impl RetryPolicy {
    pub fn from_env() -> anyhow::Result<Self> {
        let defaults = Self::default();
        let max_attempts = parse_env("JOOCODE_RETRY_ATTEMPTS")?.unwrap_or(defaults.max_attempts);
        if max_attempts == 0 {
            anyhow::bail!("JOOCODE_RETRY_ATTEMPTS must be greater than zero");
        }
        let initial_delay = Duration::from_millis(
            parse_env("JOOCODE_RETRY_INITIAL_MS")?
                .unwrap_or(defaults.initial_delay.as_millis() as usize) as u64,
        );
        let max_delay = Duration::from_millis(
            parse_env("JOOCODE_RETRY_MAX_MS")?.unwrap_or(defaults.max_delay.as_millis() as usize)
                as u64,
        );
        Ok(Self {
            max_attempts,
            initial_delay,
            max_delay,
        })
    }
}

#[derive(Clone, Debug)]
pub struct Runtime {
    inner: Arc<RuntimeInner>,
}

#[derive(Debug)]
struct RuntimeInner {
    concurrency: usize,
    default_cooldown: Duration,
    max_cooldown: Duration,
    min_interval: Duration,
    semaphores: Mutex<HashMap<String, Arc<Semaphore>>>,
    cooldowns: Mutex<HashMap<String, Instant>>,
    last_started: Mutex<HashMap<String, Instant>>,
    health: Mutex<HashMap<String, ProviderHealth>>,
}

#[derive(Clone, Debug, Default)]
struct ProviderHealth {
    consecutive_failures: u32,
    latency_ms: Option<f64>,
    requests: u64,
    failures: u64,
}

impl Runtime {
    pub fn from_env() -> anyhow::Result<Self> {
        let concurrency = parse_env("JOOCODE_PROVIDER_CONCURRENCY")?.unwrap_or(8);
        if concurrency == 0 {
            anyhow::bail!("JOOCODE_PROVIDER_CONCURRENCY must be greater than zero");
        }
        let default_cooldown = Duration::from_millis(
            parse_env("JOOCODE_PROVIDER_COOLDOWN_MS")?.unwrap_or(1_000) as u64,
        );
        let max_cooldown = Duration::from_millis(
            parse_env("JOOCODE_PROVIDER_MAX_COOLDOWN_MS")?.unwrap_or(60_000) as u64,
        );
        let min_interval = Duration::from_millis(
            parse_env("JOOCODE_PROVIDER_MIN_INTERVAL_MS")?.unwrap_or(0) as u64,
        );
        Ok(Self::with_policy(
            concurrency,
            default_cooldown,
            max_cooldown,
            min_interval,
        ))
    }

    #[cfg(test)]
    pub fn new(concurrency: usize, default_cooldown: Duration) -> Self {
        Self::with_policy(
            concurrency,
            default_cooldown,
            Duration::from_secs(60),
            Duration::ZERO,
        )
    }

    pub fn with_policy(
        concurrency: usize,
        default_cooldown: Duration,
        max_cooldown: Duration,
        min_interval: Duration,
    ) -> Self {
        Self {
            inner: Arc::new(RuntimeInner {
                concurrency,
                default_cooldown,
                max_cooldown,
                min_interval,
                semaphores: Mutex::new(HashMap::new()),
                cooldowns: Mutex::new(HashMap::new()),
                last_started: Mutex::new(HashMap::new()),
                health: Mutex::new(HashMap::new()),
            }),
        }
    }

    async fn acquire(
        &self,
        provider: &str,
        wait_for_cooldown: bool,
    ) -> Result<OwnedSemaphorePermit, SendFailure> {
        if let Some(remaining) = self.cooldown_remaining(provider).await {
            if wait_for_cooldown {
                tokio::time::sleep(remaining).await;
            } else {
                return Err(SendFailure {
                    class: FailureClass::Cooldown,
                    message: format!("provider '{provider}' is cooling down"),
                });
            }
        }
        let semaphore = {
            let mut semaphores = self.inner.semaphores.lock().await;
            semaphores
                .entry(provider.to_owned())
                .or_insert_with(|| Arc::new(Semaphore::new(self.inner.concurrency)))
                .clone()
        };
        let permit = semaphore
            .acquire_owned()
            .await
            .map_err(|error| SendFailure {
                class: FailureClass::Transport,
                message: format!("provider concurrency gate closed: {error}"),
            })?;
        if !self.inner.min_interval.is_zero() {
            let wait = {
                let mut started = self.inner.last_started.lock().await;
                let now = Instant::now();
                let wait = started
                    .get(provider)
                    .map(|last| {
                        self.inner
                            .min_interval
                            .saturating_sub(now.duration_since(*last))
                    })
                    .unwrap_or_default();
                started.insert(provider.to_owned(), now + wait);
                wait
            };
            if !wait.is_zero() {
                tokio::time::sleep(wait).await;
            }
        }
        Ok(permit)
    }

    async fn cooldown_remaining(&self, provider: &str) -> Option<Duration> {
        let mut cooldowns = self.inner.cooldowns.lock().await;
        let until = cooldowns.get(provider).copied()?;
        let now = Instant::now();
        if until <= now {
            cooldowns.remove(provider);
            None
        } else {
            Some(until.duration_since(now))
        }
    }

    async fn mark_cooldown(&self, provider: &str, duration: Option<Duration>) {
        let failures = self
            .inner
            .health
            .lock()
            .await
            .get(provider)
            .map_or(1, |health| health.consecutive_failures.max(1));
        let multiplier = 1_u32
            .checked_shl(failures.saturating_sub(1).min(10))
            .unwrap_or(u32::MAX);
        let duration = duration
            .unwrap_or_else(|| self.inner.default_cooldown.saturating_mul(multiplier))
            .min(self.inner.max_cooldown);
        if duration.is_zero() {
            return;
        }
        self.inner
            .cooldowns
            .lock()
            .await
            .insert(provider.to_owned(), Instant::now() + duration);
    }

    async fn record_result(&self, provider: &str, latency: Duration, success: bool) {
        let mut health = self.inner.health.lock().await;
        let entry = health.entry(provider.to_owned()).or_default();
        entry.requests += 1;
        let latency_ms = latency.as_secs_f64() * 1_000.0;
        entry.latency_ms = Some(
            entry
                .latency_ms
                .map_or(latency_ms, |previous| previous * 0.8 + latency_ms * 0.2),
        );
        if success {
            entry.consecutive_failures = 0;
        } else {
            entry.failures += 1;
            entry.consecutive_failures = entry.consecutive_failures.saturating_add(1);
        }
    }

    #[cfg(test)]
    pub async fn record_result_for_test(&self, provider: &str, latency: Duration, success: bool) {
        self.record_result(provider, latency, success).await;
    }

    pub async fn order_by_health<T, F>(&self, candidates: &mut [T], provider: F)
    where
        F: Fn(&T) -> &str,
    {
        let health = self.inner.health.lock().await;
        let cooldowns = self.inner.cooldowns.lock().await;
        let now = Instant::now();
        candidates.sort_by(|left, right| {
            let score = |candidate: &T| {
                let key = provider(candidate);
                let cooling = cooldowns.get(key).is_some_and(|until| *until > now);
                let state = health.get(key);
                (
                    cooling,
                    state.map_or(u32::MAX, |value| value.consecutive_failures),
                    state.and_then(|value| value.latency_ms).unwrap_or(f64::MAX),
                )
            };
            let left = score(left);
            let right = score(right);
            left.0
                .cmp(&right.0)
                .then(left.1.cmp(&right.1))
                .then_with(|| left.2.total_cmp(&right.2))
        });
    }

    pub async fn provider_statuses(&self, providers: &[String]) -> Vec<ProviderStatus> {
        let now = Instant::now();
        let semaphores = self.inner.semaphores.lock().await;
        let cooldowns = self.inner.cooldowns.lock().await;
        let health = self.inner.health.lock().await;
        providers
            .iter()
            .map(|provider| {
                let available = semaphores
                    .get(provider)
                    .map_or(self.inner.concurrency, |semaphore| {
                        semaphore.available_permits()
                    });
                let cooldown_ms = cooldowns
                    .get(provider)
                    .and_then(|until| (*until > now).then(|| until.duration_since(now)))
                    .map_or(0, |remaining| remaining.as_millis() as u64);
                ProviderStatus {
                    provider: provider.clone(),
                    state: if cooldown_ms > 0 {
                        "cooling_down"
                    } else if available == 0 {
                        "saturated"
                    } else {
                        "available"
                    },
                    active_requests: self.inner.concurrency.saturating_sub(available),
                    concurrency_limit: self.inner.concurrency,
                    cooldown_ms,
                    latency_ms: health.get(provider).and_then(|health| health.latency_ms),
                    consecutive_failures: health
                        .get(provider)
                        .map_or(0, |health| health.consecutive_failures),
                }
            })
            .collect()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ProviderStatus {
    pub provider: String,
    pub state: &'static str,
    pub active_requests: usize,
    pub concurrency_limit: usize,
    pub cooldown_ms: u64,
    pub latency_ms: Option<f64>,
    pub consecutive_failures: u32,
}

#[derive(Debug)]
pub struct SendFailure {
    pub class: FailureClass,
    pub message: String,
}

#[derive(Clone, Copy)]
pub struct RouteBudget<'a> {
    pub runtime: &'a Runtime,
    pub provider: &'a str,
    pub wait_for_cooldown: bool,
}

pub struct UpstreamResponse {
    response: Response,
    _permit: OwnedSemaphorePermit,
}

impl UpstreamResponse {
    pub fn status(&self) -> StatusCode {
        self.response.status()
    }

    pub fn headers(&self) -> &HeaderMap {
        self.response.headers()
    }

    pub async fn text(self) -> Result<String, reqwest::Error> {
        self.response.text().await
    }

    pub async fn json<T: serde::de::DeserializeOwned>(self) -> Result<T, reqwest::Error> {
        self.response.json().await
    }

    pub fn bytes_stream(self) -> Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>> {
        let UpstreamResponse {
            response,
            _permit: permit,
        } = self;
        let stream = async_stream::stream! {
            let _permit = permit;
            let mut upstream = Box::pin(response.bytes_stream());
            while let Some(item) = upstream.next().await {
                yield item;
            }
        };
        Box::pin(stream)
    }
}

pub async fn send_json(
    client: &Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
    policy: &RetryPolicy,
    budget: RouteBudget<'_>,
) -> Result<UpstreamResponse, SendFailure> {
    let attempts = policy.max_attempts.max(1);
    let mut delay = policy.initial_delay;
    let permit = budget
        .runtime
        .acquire(budget.provider, budget.wait_for_cooldown)
        .await?;
    for attempt in 1..=attempts {
        let started = Instant::now();
        match client
            .post(url)
            .headers(headers.clone())
            .json(body)
            .send()
            .await
        {
            Ok(response) => {
                let class = classify_status(response.status());
                let success = response.status().is_success();
                budget
                    .runtime
                    .record_result(budget.provider, started.elapsed(), success)
                    .await;
                if attempt < attempts && retry_same_provider(class) {
                    let wait = retry_after(&response)
                        .unwrap_or(delay)
                        .min(policy.max_delay);
                    tokio::time::sleep(wait).await;
                    delay = delay.saturating_mul(2).min(policy.max_delay);
                    continue;
                }
                if failover_eligible(class) {
                    budget
                        .runtime
                        .mark_cooldown(budget.provider, retry_after(&response))
                        .await;
                }
                return Ok(UpstreamResponse {
                    response,
                    _permit: permit,
                });
            }
            Err(error) => {
                let class = if error.is_timeout() {
                    FailureClass::Timeout
                } else {
                    FailureClass::Transport
                };
                budget
                    .runtime
                    .record_result(budget.provider, started.elapsed(), false)
                    .await;
                if attempt < attempts {
                    tokio::time::sleep(delay.min(policy.max_delay)).await;
                    delay = delay.saturating_mul(2).min(policy.max_delay);
                    continue;
                }
                budget.runtime.mark_cooldown(budget.provider, None).await;
                return Err(SendFailure {
                    class,
                    message: error.to_string(),
                });
            }
        }
    }
    unreachable!("at least one upstream attempt is always made")
}

pub fn classify_status(status: StatusCode) -> FailureClass {
    match status.as_u16() {
        401 | 403 => FailureClass::Authentication,
        408 => FailureClass::Timeout,
        429 => FailureClass::RateLimited,
        500 | 502 | 503 | 504 => FailureClass::Capacity,
        400..=499 => FailureClass::InvalidRequest,
        _ => FailureClass::Provider,
    }
}

pub fn retry_same_provider(class: FailureClass) -> bool {
    matches!(
        class,
        FailureClass::RateLimited
            | FailureClass::Capacity
            | FailureClass::Timeout
            | FailureClass::Transport
    )
}

pub fn failover_eligible(class: FailureClass) -> bool {
    matches!(
        class,
        FailureClass::Authentication
            | FailureClass::RateLimited
            | FailureClass::Capacity
            | FailureClass::Timeout
            | FailureClass::Transport
            | FailureClass::Cooldown
    )
}

fn retry_after(response: &Response) -> Option<Duration> {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

fn parse_env(name: &str) -> anyhow::Result<Option<usize>> {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("{name} must be a non-negative integer"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, http::StatusCode, routing::post};
    use serde_json::json;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[test]
    fn classifies_retry_and_failover_statuses() {
        assert_eq!(
            classify_status(StatusCode::UNAUTHORIZED),
            FailureClass::Authentication
        );
        assert!(failover_eligible(FailureClass::Authentication));
        assert!(!retry_same_provider(FailureClass::Authentication));
        assert!(retry_same_provider(FailureClass::RateLimited));
        assert!(!failover_eligible(FailureClass::InvalidRequest));
    }

    #[tokio::test]
    async fn retries_retryable_statuses() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let state = attempts.clone();
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move || {
                let state = state.clone();
                async move {
                    if state.fetch_add(1, Ordering::SeqCst) == 0 {
                        (
                            StatusCode::SERVICE_UNAVAILABLE,
                            [(reqwest::header::RETRY_AFTER.as_str(), "0")],
                            Json(json!({"error":"busy"})),
                        )
                    } else {
                        (
                            StatusCode::OK,
                            [(reqwest::header::RETRY_AFTER.as_str(), "0")],
                            Json(json!({"ok":true})),
                        )
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let response = send_json(
            &Client::new(),
            &format!("http://{address}/v1/chat/completions"),
            &HeaderMap::new(),
            &json!({}),
            &RetryPolicy {
                max_attempts: 2,
                initial_delay: Duration::from_secs(5),
                max_delay: Duration::from_secs(5),
            },
            RouteBudget {
                runtime: &Runtime::new(8, Duration::ZERO),
                provider: "fixture",
                wait_for_cooldown: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn cooldown_can_skip_combo_candidates() {
        let runtime = Runtime::new(1, Duration::from_secs(60));
        runtime.mark_cooldown("busy", None).await;
        let failure = runtime.acquire("busy", false).await.unwrap_err();
        assert_eq!(failure.class, FailureClass::Cooldown);
    }

    #[tokio::test]
    async fn provider_status_reports_cooldown_and_capacity() {
        let runtime = Runtime::new(2, Duration::from_secs(1));
        runtime.mark_cooldown("busy", None).await;
        let statuses = runtime
            .provider_statuses(&["busy".into(), "idle".into()])
            .await;
        assert_eq!(statuses[0].state, "cooling_down");
        assert!(statuses[0].cooldown_ms > 0);
        assert_eq!(statuses[1].state, "available");
        assert_eq!(statuses[1].active_requests, 0);
        assert_eq!(statuses[1].concurrency_limit, 2);
    }

    #[tokio::test]
    async fn concurrency_permit_is_held_until_the_body_finishes() {
        let app = Router::new().route(
            "/v1/chat/completions",
            post(|| async { Json(json!({"ok":true})) }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let runtime = Runtime::new(1, Duration::ZERO);
        let response = send_json(
            &Client::new(),
            &format!("http://{address}/v1/chat/completions"),
            &HeaderMap::new(),
            &json!({}),
            &RetryPolicy {
                max_attempts: 1,
                initial_delay: Duration::ZERO,
                max_delay: Duration::ZERO,
            },
            RouteBudget {
                runtime: &runtime,
                provider: "fixture",
                wait_for_cooldown: true,
            },
        )
        .await
        .unwrap();
        assert_eq!(
            runtime
                .inner
                .semaphores
                .lock()
                .await
                .get("fixture")
                .unwrap()
                .available_permits(),
            0
        );
        drop(response);
        assert_eq!(
            runtime
                .inner
                .semaphores
                .lock()
                .await
                .get("fixture")
                .unwrap()
                .available_permits(),
            1
        );
    }
    #[tokio::test]
    async fn provider_pacing_delays_back_to_back_requests() {
        let runtime = Runtime::with_policy(
            2,
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_millis(30),
        );
        let first = runtime.acquire("fixture", true).await.unwrap();
        drop(first);
        let started = Instant::now();
        let second = runtime.acquire("fixture", true).await.unwrap();
        assert!(started.elapsed() >= Duration::from_millis(20));
        drop(second);
    }

    #[tokio::test]
    async fn latency_ordering_prefers_healthy_fast_provider() {
        let runtime = Runtime::new(2, Duration::ZERO);
        runtime
            .record_result_for_test("slow", Duration::from_millis(100), true)
            .await;
        runtime
            .record_result_for_test("fast", Duration::from_millis(10), true)
            .await;
        let mut candidates = vec!["slow".to_owned(), "fast".to_owned()];
        runtime
            .order_by_health(&mut candidates, String::as_str)
            .await;
        assert_eq!(candidates, ["fast", "slow"]);
    }

    #[tokio::test]
    async fn adaptive_cooldown_tracks_failure_streaks() {
        let runtime = Runtime::with_policy(
            1,
            Duration::from_millis(10),
            Duration::from_secs(1),
            Duration::ZERO,
        );
        runtime
            .record_result_for_test("fixture", Duration::from_millis(1), false)
            .await;
        runtime.mark_cooldown("fixture", None).await;
        let first = runtime.cooldown_remaining("fixture").await.unwrap();
        runtime
            .record_result_for_test("fixture", Duration::from_millis(1), false)
            .await;
        runtime.mark_cooldown("fixture", None).await;
        let second = runtime.cooldown_remaining("fixture").await.unwrap();
        assert!(second > first);
        runtime
            .record_result_for_test("fixture", Duration::from_millis(1), true)
            .await;
        let status = runtime.provider_statuses(&["fixture".into()]).await;
        assert_eq!(status[0].consecutive_failures, 0);
    }
}
