use std::{collections::BTreeMap, sync::Arc, time::Duration};

use anyhow::Context;
use reqwest::{
    Client,
    header::{HeaderMap, HeaderValue},
};
use serde::Serialize;
use tokio::sync::Mutex;

#[cfg(test)]
use crate::config::ConfigPaths;
use crate::sources::{DiscoveredCatalog, SourceSelection};

#[derive(Clone, Debug, Serialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub upstream_id: String,
    pub name: String,
    pub reasoning: bool,
    pub context_window: Option<u64>,
    pub max_output_tokens: Option<u64>,
}

#[derive(Clone, Debug)]
pub enum Credential {
    None,
    Bearer(String),
    Copilot(CopilotCredential),
}

#[derive(Clone, Debug)]
pub struct CopilotCredential {
    raw_token: Arc<str>,
    exchange: Arc<Mutex<Option<CopilotExchange>>>,
}

#[derive(Clone, Debug)]
struct CopilotExchange {
    token: String,
    base_url: Option<String>,
    expires_at: u64,
}

impl CopilotCredential {
    pub fn new(raw_token: String) -> Self {
        Self {
            raw_token: raw_token.into(),
            exchange: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn exchange(&self, client: &Client) -> anyhow::Result<(String, Option<String>)> {
        let now = unix_timestamp();
        let mut cached = self.exchange.lock().await;
        if let Some(exchange) = cached.as_ref()
            && exchange.expires_at > now + 120
        {
            return Ok((exchange.token.clone(), exchange.base_url.clone()));
        }

        let response = client
            .get("https://api.github.com/copilot_internal/v2/token")
            .header("authorization", format!("token {}", self.raw_token))
            .header("accept", "application/json")
            .header("editor-version", "vscode/1.104.1")
            .header("user-agent", "GitHubCopilotChat/0.26.7")
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .context("GitHub Copilot token exchange failed")?
            .error_for_status()
            .context("GitHub rejected the Copilot token exchange")?
            .json::<serde_json::Value>()
            .await
            .context("invalid Copilot token exchange response")?;
        let token = response
            .get("token")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.is_empty())
            .context("Copilot token exchange returned no token")?
            .to_owned();
        let expires_at = response
            .get("expires_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(now + 1_800);
        let base_url = response
            .pointer("/endpoints/api")
            .and_then(serde_json::Value::as_str)
            .map(|value| value.trim_end_matches('/').to_owned())
            .or_else(|| copilot_base_url_from_token(&token));

        *cached = Some(CopilotExchange {
            token: token.clone(),
            base_url: base_url.clone(),
            expires_at,
        });
        Ok((token, base_url))
    }
}

fn unix_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn copilot_base_url_from_token(token: &str) -> Option<String> {
    let endpoint = token
        .split(';')
        .find_map(|part| part.trim().strip_prefix("proxy-ep="))?
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let host = endpoint
        .strip_prefix("proxy.")
        .map(|suffix| format!("api.{suffix}"))
        .unwrap_or_else(|| endpoint.to_owned());
    Some(format!("https://{host}"))
}

#[derive(Clone, Debug)]
pub struct Provider {
    pub base_url: String,
    pub credential: Credential,
    pub headers: HeaderMap,
}

impl Provider {
    pub async fn request_parts(&self, client: &Client) -> anyhow::Result<(String, HeaderMap)> {
        let mut headers = self.headers.clone();
        let mut base_url = self.base_url.clone();
        match &self.credential {
            Credential::None => {}
            Credential::Bearer(token) => insert_bearer(&mut headers, token)?,
            Credential::Copilot(credential) => {
                let (token, discovered_base_url) = credential.exchange(client).await?;
                insert_bearer(&mut headers, &token)?;
                if let Some(discovered_base_url) = discovered_base_url {
                    base_url = discovered_base_url;
                }
            }
        }
        Ok((base_url, headers))
    }
}

fn insert_bearer(headers: &mut HeaderMap, token: &str) -> anyhow::Result<()> {
    let value =
        HeaderValue::from_str(&format!("Bearer {token}")).context("invalid provider credential")?;
    headers.insert("authorization", value);
    Ok(())
}

#[derive(Clone, Debug)]
struct Route {
    provider_key: String,
    upstream_id: String,
}

#[derive(Clone, Debug)]
pub struct SourceReport {
    pub source: String,
    pub status: &'static str,
    pub providers: usize,
    pub models: usize,
    pub detail: Option<String>,
}

#[derive(Clone)]
pub struct Registry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    client: Client,
    providers: BTreeMap<String, Provider>,
    routes: BTreeMap<String, Route>,
    models: Vec<ModelInfo>,
    source_reports: Vec<SourceReport>,
}

impl Registry {
    #[cfg(test)]
    pub fn load(paths: &ConfigPaths) -> anyhow::Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("joocode/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let catalog = crate::sources::load_opencode_catalog("opencode", None, paths);
        Self::from_catalogs(client, vec![catalog])
    }

    pub async fn discover(selection: &SourceSelection) -> anyhow::Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("joocode/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let catalogs = crate::sources::discover(selection, &client).await;
        Self::from_catalogs(client, catalogs)
    }

    fn from_catalogs(
        client: Client,
        catalogs: Vec<anyhow::Result<DiscoveredCatalog>>,
    ) -> anyhow::Result<Self> {
        let mut providers = BTreeMap::new();
        let mut routes = BTreeMap::new();
        let mut models = Vec::new();
        let mut source_reports = Vec::new();

        for result in catalogs {
            match result {
                Ok(catalog) => {
                    let provider_count = catalog.providers.len();
                    let model_count = catalog
                        .providers
                        .iter()
                        .map(|provider| provider.models.len())
                        .sum();
                    for discovered in catalog.providers {
                        for model in discovered.models {
                            if routes.contains_key(&model.info.id) {
                                continue;
                            }
                            routes.insert(
                                model.info.id.clone(),
                                Route {
                                    provider_key: discovered.key.clone(),
                                    upstream_id: model.info.upstream_id.clone(),
                                },
                            );
                            models.push(model.info);
                        }
                        providers.insert(discovered.key, discovered.provider);
                    }
                    source_reports.push(SourceReport {
                        source: catalog.source,
                        status: if model_count == 0 {
                            "missing"
                        } else {
                            "loaded"
                        },
                        providers: provider_count,
                        models: model_count,
                        detail: catalog.detail,
                    });
                }
                Err(error) => {
                    let rendered = error.to_string();
                    let source = rendered
                        .strip_prefix("source:")
                        .unwrap_or("unknown")
                        .to_owned();
                    source_reports.push(SourceReport {
                        source,
                        status: "error",
                        providers: 0,
                        models: 0,
                        detail: Some(
                            error
                                .chain()
                                .skip(1)
                                .map(ToString::to_string)
                                .collect::<Vec<_>>()
                                .join(": "),
                        ),
                    });
                }
            }
        }

        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self {
            inner: Arc::new(RegistryInner {
                client,
                providers,
                routes,
                models,
                source_reports,
            }),
        })
    }

    pub fn client(&self) -> &Client {
        &self.inner.client
    }
    pub fn models(&self) -> &[ModelInfo] {
        &self.inner.models
    }
    pub fn source_reports(&self) -> &[SourceReport] {
        &self.inner.source_reports
    }
    pub fn provider_count(&self) -> usize {
        self.inner.providers.len()
    }

    pub fn resolve(&self, model: &str) -> anyhow::Result<(&Provider, String)> {
        let route = self
            .inner
            .routes
            .get(model)
            .with_context(|| format!("unknown model '{model}'"))?;
        let provider = self
            .inner
            .providers
            .get(&route.provider_key)
            .with_context(|| format!("provider route for '{model}' is unavailable"))?;
        Ok((provider, route.upstream_id.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::{DiscoveredModel, DiscoveredProvider};

    #[test]
    fn derives_enterprise_copilot_api_host() {
        assert_eq!(
            copilot_base_url_from_token("tid=x;proxy-ep=proxy.example.githubcopilot.com;exp=1")
                .as_deref(),
            Some("https://api.example.githubcopilot.com")
        );
    }

    #[test]
    fn resolves_source_namespaced_model_ids() {
        let model = ModelInfo {
            id: "hermes/local/model-a".into(),
            provider: "hermes/local".into(),
            upstream_id: "model-a".into(),
            name: "Model A".into(),
            reasoning: false,
            context_window: None,
            max_output_tokens: None,
        };
        let catalog = DiscoveredCatalog {
            source: "hermes".into(),
            detail: None,
            providers: vec![DiscoveredProvider {
                key: "hermes:local".into(),
                provider: Provider {
                    base_url: "https://example.test/v1".into(),
                    credential: Credential::None,
                    headers: HeaderMap::new(),
                },
                models: vec![DiscoveredModel { info: model }],
            }],
        };
        let registry = Registry::from_catalogs(Client::new(), vec![Ok(catalog)]).unwrap();
        let (provider, upstream_id) = registry.resolve("hermes/local/model-a").unwrap();
        assert_eq!(provider.base_url, "https://example.test/v1");
        assert_eq!(upstream_id, "model-a");
    }
}
