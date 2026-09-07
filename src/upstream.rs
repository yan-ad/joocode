use std::time::Duration;

use reqwest::{Client, Response, StatusCode, header::HeaderMap};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureClass {
    Authentication,
    RateLimited,
    Capacity,
    Timeout,
    Transport,
    InvalidRequest,
    Provider,
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

fn parse_env(name: &str) -> anyhow::Result<Option<usize>> {
    std::env::var(name)
        .ok()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|error| anyhow::anyhow!("{name} must be a non-negative integer: {error}"))
        })
        .transpose()
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

#[derive(Debug)]
pub struct SendFailure {
    pub class: FailureClass,
    pub message: String,
}

pub async fn send_json(
    client: &Client,
    url: &str,
    headers: &HeaderMap,
    body: &Value,
    policy: &RetryPolicy,
) -> Result<Response, SendFailure> {
    let attempts = policy.max_attempts.max(1);
    let mut delay = policy.initial_delay;
    for attempt in 1..=attempts {
        match client
            .post(url)
            .headers(headers.clone())
            .json(body)
            .send()
            .await
        {
            Ok(response) => {
                let class = classify_status(response.status());
                if attempt < attempts && retry_same_provider(class) {
                    let wait = retry_after(&response)
                        .unwrap_or(delay)
                        .min(policy.max_delay);
                    tokio::time::sleep(wait).await;
                    delay = delay.saturating_mul(2).min(policy.max_delay);
                    continue;
                }
                return Ok(response);
            }
            Err(error) => {
                let class = if error.is_timeout() {
                    FailureClass::Timeout
                } else {
                    FailureClass::Transport
                };
                if attempt < attempts {
                    tokio::time::sleep(delay.min(policy.max_delay)).await;
                    delay = delay.saturating_mul(2).min(policy.max_delay);
                    continue;
                }
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
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
    }
}
