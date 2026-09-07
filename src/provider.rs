use std::{collections::BTreeMap, sync::Arc, time::Duration};

use anyhow::Context;
use reqwest::{
    Client,
    header::{HeaderMap, HeaderValue},
};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
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

/// The upstream HTTP API spoken by a provider route.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WireApi {
    #[default]
    #[serde(alias = "chat", alias = "chat_completions", alias = "openai-chat")]
    OpenAiChat,
    #[serde(alias = "responses", alias = "openai-responses")]
    OpenAiResponses,
    #[serde(alias = "anthropic", alias = "messages", alias = "anthropic-messages")]
    AnthropicMessages,
    #[serde(alias = "google", alias = "gemini-native")]
    Gemini,
}

impl WireApi {
    pub const fn endpoint(self) -> &'static str {
        match self {
            Self::OpenAiChat => "chat/completions",
            Self::OpenAiResponses => "responses",
            Self::AnthropicMessages => "messages",
            Self::Gemini => "models",
        }
    }
}

struct ComboRoute {
    routes: Vec<Route>,
    weights: Vec<u32>,
    strategy: crate::combo::Strategy,
    cursor: AtomicU64,
}

#[derive(Clone)]
pub struct RegistryStore {
    inner: Arc<std::sync::RwLock<Registry>>,
}

impl RegistryStore {
    pub fn new(registry: Registry) -> Self {
        Self {
            inner: Arc::new(std::sync::RwLock::new(registry)),
        }
    }

    pub fn snapshot(&self) -> Registry {
        self.inner.read().expect("registry lock poisoned").clone()
    }

    pub fn replace(&self, registry: Registry) {
        *self.inner.write().expect("registry lock poisoned") = registry;
    }
}

impl ComboRoute {
    fn start_index(&self) -> usize {
        match self.strategy {
            crate::combo::Strategy::Failover | crate::combo::Strategy::LowestLatency => 0,
            crate::combo::Strategy::WeightedRoundRobin => {
                let total = self
                    .weights
                    .iter()
                    .map(|weight| u64::from(*weight))
                    .sum::<u64>();
                let position = self.cursor.fetch_add(1, Ordering::Relaxed) % total;
                let mut boundary = 0_u64;
                self.weights
                    .iter()
                    .position(|weight| {
                        boundary += u64::from(*weight);
                        position < boundary
                    })
                    .unwrap_or(0)
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Credential {
    None,
    Bearer(String),
    BearerPool(Arc<BearerPool>),
    Copilot(CopilotCredential),
}

#[derive(Debug)]
pub struct BearerPool {
    keys: Vec<String>,
    cursor: AtomicU64,
}

impl BearerPool {
    pub fn new(keys: Vec<String>) -> Option<Self> {
        (!keys.is_empty()).then(|| Self {
            keys,
            cursor: AtomicU64::new(0),
        })
    }

    fn next(&self) -> &str {
        let index = self.cursor.fetch_add(1, Ordering::Relaxed) as usize % self.keys.len();
        &self.keys[index]
    }
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
    pub wire_api: WireApi,
}

impl Provider {
    pub async fn request_parts(&self, client: &Client) -> anyhow::Result<(String, HeaderMap)> {
        let mut headers = self.headers.clone();
        let mut base_url = self.base_url.clone();
        match &self.credential {
            Credential::None => {}
            Credential::Bearer(token) => insert_bearer(&mut headers, token)?,
            Credential::BearerPool(pool) => insert_bearer(&mut headers, pool.next())?,
            Credential::Copilot(credential) => {
                let (token, discovered_base_url) = credential.exchange(client).await?;
                insert_bearer(&mut headers, &token)?;
                if let Some(discovered_base_url) = discovered_base_url {
                    base_url = discovered_base_url;
                }
            }
        }
        if self.wire_api == WireApi::AnthropicMessages {
            if let Some(authorization) = headers.remove("authorization")
                && !headers.contains_key("x-api-key")
            {
                let token = authorization
                    .to_str()
                    .ok()
                    .and_then(|value| value.strip_prefix("Bearer "))
                    .unwrap_or_default();
                if !token.is_empty() {
                    headers.insert("x-api-key", HeaderValue::from_str(token)?);
                }
            }
            if !headers.contains_key("anthropic-version") {
                headers.insert("anthropic-version", HeaderValue::from_static("2023-06-01"));
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
    wire_api: WireApi,
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
    combos: BTreeMap<String, ComboRoute>,
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
        let disabled = crate::target_config::TargetPreferences::load()?.disabled_models;
        Self::from_catalogs_with_disabled(client, vec![catalog], &disabled)
    }

    pub async fn discover(selection: &SourceSelection) -> anyhow::Result<Self> {
        let client = Client::builder()
            .user_agent(concat!("joocode/", env!("CARGO_PKG_VERSION")))
            .build()?;
        let catalogs = crate::sources::discover(selection, &client).await;
        let disabled = crate::target_config::TargetPreferences::load()?.disabled_models;
        Self::from_catalogs_with_disabled(client, catalogs, &disabled)
    }

    #[cfg(test)]
    pub(crate) fn from_catalogs_and_combos_for_test(
        client: Client,
        catalogs: Vec<anyhow::Result<DiscoveredCatalog>>,
        combos: Vec<crate::combo::Combo>,
    ) -> anyhow::Result<Self> {
        Self::from_catalogs_combos_and_disabled(
            client,
            catalogs,
            Ok(combos),
            &std::collections::BTreeSet::new(),
        )
    }

    fn from_catalogs_with_disabled(
        client: Client,
        catalogs: Vec<anyhow::Result<DiscoveredCatalog>>,
        disabled_models: &std::collections::BTreeSet<String>,
    ) -> anyhow::Result<Self> {
        let combos = crate::combo::load();
        Self::from_catalogs_combos_and_disabled(client, catalogs, combos, disabled_models)
    }

    fn from_catalogs_combos_and_disabled(
        client: Client,
        catalogs: Vec<anyhow::Result<DiscoveredCatalog>>,
        configured_combos: anyhow::Result<Vec<crate::combo::Combo>>,
        disabled_models: &std::collections::BTreeSet<String>,
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
                        .flat_map(|provider| &provider.models)
                        .filter(|model| !disabled_models.contains(&model.info.id))
                        .count();
                    for discovered in catalog.providers {
                        for model in discovered.models {
                            if disabled_models.contains(&model.info.id) {
                                continue;
                            }
                            if routes.contains_key(&model.info.id) {
                                continue;
                            }
                            routes.insert(
                                model.info.id.clone(),
                                Route {
                                    provider_key: discovered.key.clone(),
                                    upstream_id: model.info.upstream_id.clone(),
                                    wire_api: discovered.wire_api,
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

        let mut combos = BTreeMap::new();
        match configured_combos {
            Ok(configured) => {
                let mut combo_models = 0;
                for combo in configured {
                    let candidates = combo
                        .models
                        .iter()
                        .filter_map(|model| routes.get(model.model()).cloned())
                        .collect::<Vec<_>>();
                    if candidates.len() != combo.models.len() {
                        source_reports.push(SourceReport {
                            source: format!("combo/{}", combo.name),
                            status: "error",
                            providers: 0,
                            models: 0,
                            detail: Some("one or more combo models are unavailable".into()),
                        });
                        continue;
                    }
                    let transports = candidates
                        .iter()
                        .map(|route| route.wire_api)
                        .collect::<std::collections::BTreeSet<_>>();
                    if transports.len() > 1 {
                        source_reports.push(SourceReport {
                            source: format!("combo/{}", combo.name),
                            status: "error",
                            providers: 0,
                            models: 0,
                            detail: Some("combo mixes incompatible wire APIs; configure a common compatible transport explicitly".into()),
                        });
                        continue;
                    }
                    let id = format!("combo/{}", combo.name);
                    if disabled_models.contains(&id) {
                        continue;
                    }
                    combos.insert(
                        id.clone(),
                        ComboRoute {
                            routes: candidates,
                            weights: combo
                                .models
                                .iter()
                                .map(crate::combo::ComboModel::weight)
                                .collect(),
                            strategy: combo.strategy,
                            cursor: AtomicU64::new(0),
                        },
                    );
                    models.push(ModelInfo {
                        id,
                        provider: "combo".into(),
                        upstream_id: combo.name.clone(),
                        name: format!(
                            "{} ({})",
                            combo.name,
                            match combo.strategy {
                                crate::combo::Strategy::Failover => "failover",
                                crate::combo::Strategy::WeightedRoundRobin => "weighted",
                                crate::combo::Strategy::LowestLatency => "lowest latency",
                            }
                        ),
                        reasoning: combo.models.iter().any(|model| {
                            models
                                .iter()
                                .any(|info| info.id == model.model() && info.reasoning)
                        }),
                        context_window: combo
                            .models
                            .iter()
                            .filter_map(|model| {
                                models
                                    .iter()
                                    .find(|info| info.id == model.model())
                                    .and_then(|info| info.context_window)
                            })
                            .min(),
                        max_output_tokens: combo
                            .models
                            .iter()
                            .filter_map(|model| {
                                models
                                    .iter()
                                    .find(|info| info.id == model.model())
                                    .and_then(|info| info.max_output_tokens)
                            })
                            .min(),
                    });
                    combo_models += 1;
                }
                if combo_models > 0 {
                    source_reports.push(SourceReport {
                        source: "combos".into(),
                        status: "loaded",
                        providers: 0,
                        models: combo_models,
                        detail: crate::combo::path()
                            .ok()
                            .map(|path| path.display().to_string()),
                    });
                }
            }
            Err(error) => source_reports.push(SourceReport {
                source: "combos".into(),
                status: "error",
                providers: 0,
                models: 0,
                detail: Some(error.to_string()),
            }),
        }

        models.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(Self {
            inner: Arc::new(RegistryInner {
                client,
                providers,
                routes,
                combos,
                models,
                source_reports,
            }),
        })
    }

    #[cfg(test)]
    pub(crate) fn from_catalogs_for_test(
        client: Client,
        catalogs: Vec<anyhow::Result<DiscoveredCatalog>>,
    ) -> anyhow::Result<Self> {
        Self::from_catalogs_combos_and_disabled(
            client,
            catalogs,
            Ok(Vec::new()),
            &std::collections::BTreeSet::new(),
        )
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

    pub fn combo_strategy(&self, model: &str) -> Option<crate::combo::Strategy> {
        self.inner.combos.get(model).map(|combo| combo.strategy)
    }

    pub fn provider_keys(&self) -> Vec<String> {
        self.inner.providers.keys().cloned().collect()
    }

    pub fn resolve(&self, model: &str) -> anyhow::Result<(&Provider, String)> {
        let route = self
            .inner
            .routes
            .get(model)
            .or_else(|| {
                self.inner
                    .combos
                    .get(model)
                    .and_then(|combo| combo.routes.first())
            })
            .with_context(|| format!("unknown model '{model}'"))?;
        let provider = self
            .inner
            .providers
            .get(&route.provider_key)
            .with_context(|| format!("provider route for '{model}' is unavailable"))?;
        Ok((provider, route.upstream_id.clone()))
    }

    pub fn resolve_candidates(
        &self,
        model: &str,
    ) -> anyhow::Result<Vec<(String, Provider, String, WireApi)>> {
        if let Some(combo) = self.inner.combos.get(model) {
            let start = combo.start_index();
            return combo
                .routes
                .iter()
                .cycle()
                .skip(start)
                .take(combo.routes.len())
                .map(|route| {
                    let provider = self
                        .inner
                        .providers
                        .get(&route.provider_key)
                        .with_context(|| format!("provider route for '{model}' is unavailable"))?;
                    Ok((
                        route.provider_key.clone(),
                        provider.clone(),
                        route.upstream_id.clone(),
                        route.wire_api,
                    ))
                })
                .collect();
        }
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
        Ok(vec![(
            route.provider_key.clone(),
            provider.clone(),
            route.upstream_id.clone(),
            route.wire_api,
        )])
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
                wire_api: WireApi::OpenAiChat,
                key: "hermes:local".into(),
                provider: Provider {
                    base_url: "https://example.test/v1".into(),
                    credential: Credential::None,
                    headers: HeaderMap::new(),
                    wire_api: WireApi::OpenAiChat,
                },
                models: vec![DiscoveredModel { info: model }],
            }],
        };
        let registry = Registry::from_catalogs_for_test(Client::new(), vec![Ok(catalog)]).unwrap();
        let (provider, upstream_id) = registry.resolve("hermes/local/model-a").unwrap();
        assert_eq!(provider.base_url, "https://example.test/v1");
        assert_eq!(upstream_id, "model-a");
    }

    #[test]
    fn weighted_combo_rotates_primary_candidate_by_weight() {
        use crate::combo::{Combo, ComboModel, Strategy};

        let discovered = |key: &str, model: &str| DiscoveredProvider {
            wire_api: crate::provider::WireApi::OpenAiChat,
            key: key.into(),
            provider: Provider {
                base_url: format!("https://{key}.example/v1"),
                credential: Credential::None,
                headers: HeaderMap::new(),
                wire_api: WireApi::OpenAiChat,
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
            Client::new(),
            vec![Ok(DiscoveredCatalog {
                source: "fixture".into(),
                detail: None,
                providers: vec![discovered("a", "model"), discovered("b", "model")],
            })],
            vec![Combo {
                name: "balanced".into(),
                strategy: Strategy::WeightedRoundRobin,
                models: vec![
                    ComboModel::Weighted {
                        model: "a/model".into(),
                        weight: 3,
                    },
                    ComboModel::Weighted {
                        model: "b/model".into(),
                        weight: 1,
                    },
                ],
            }],
        )
        .unwrap();

        let primaries = (0..8)
            .map(|_| {
                registry.resolve_candidates("combo/balanced").unwrap()[0]
                    .0
                    .clone()
            })
            .collect::<Vec<_>>();
        assert_eq!(primaries, vec!["a", "a", "a", "b", "a", "a", "a", "b"]);
    }

    #[tokio::test]
    async fn bearer_pool_rotates_round_robin() {
        let provider = Provider {
            base_url: "https://example.test/v1".into(),
            credential: Credential::BearerPool(Arc::new(
                BearerPool::new(vec!["a".into(), "b".into()]).unwrap(),
            )),
            headers: HeaderMap::new(),
            wire_api: WireApi::OpenAiChat,
        };
        let mut values = Vec::new();
        for _ in 0..4 {
            let (_, headers) = provider.request_parts(&Client::new()).await.unwrap();
            values.push(headers["authorization"].to_str().unwrap().to_owned());
        }
        assert_eq!(values, ["Bearer a", "Bearer b", "Bearer a", "Bearer b"]);
    }

    #[test]
    fn exact_disabled_model_ids_are_filtered() {
        let discovered = |id: &str| DiscoveredModel {
            info: ModelInfo {
                id: id.into(),
                provider: "fixture".into(),
                upstream_id: id.into(),
                name: id.into(),
                reasoning: false,
                context_window: None,
                max_output_tokens: None,
            },
        };
        let catalog = DiscoveredCatalog {
            source: "fixture".into(),
            detail: None,
            providers: vec![DiscoveredProvider {
                wire_api: WireApi::OpenAiChat,
                key: "fixture".into(),
                provider: Provider {
                    base_url: "https://example.test/v1".into(),
                    credential: Credential::None,
                    headers: HeaderMap::new(),
                    wire_api: WireApi::OpenAiChat,
                },
                models: vec![
                    discovered("provider/model"),
                    discovered("provider/model-extra"),
                ],
            }],
        };
        let disabled = std::collections::BTreeSet::from(["provider/model".to_owned()]);
        let registry = Registry::from_catalogs_combos_and_disabled(
            Client::new(),
            vec![Ok(catalog)],
            Ok(Vec::new()),
            &disabled,
        )
        .unwrap();
        assert!(registry.resolve("provider/model").is_err());
        assert!(registry.resolve("provider/model-extra").is_ok());
        assert_eq!(
            registry
                .models()
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["provider/model-extra"]
        );
    }
}
