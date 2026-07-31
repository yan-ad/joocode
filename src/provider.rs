use std::{collections::BTreeMap, sync::Arc};

use anyhow::{Context, bail};
use reqwest::{
    Client,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::Serialize;

use crate::config::{self, AuthEntry, ConfigPaths, ModelConfig};

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
pub struct Provider {
    pub base_url: String,
    pub credential: Option<String>,
    pub headers: HeaderMap,
    pub models: BTreeMap<String, ModelConfig>,
}

impl Provider {
    pub fn chat_completions_url(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

#[derive(Clone)]
pub struct Registry {
    inner: Arc<RegistryInner>,
}

struct RegistryInner {
    client: Client,
    providers: BTreeMap<String, Provider>,
    models: Vec<ModelInfo>,
}

impl Registry {
    pub fn load(paths: &ConfigPaths) -> anyhow::Result<Self> {
        let (config, auth) = config::load(paths)?;
        let mut providers = BTreeMap::new();
        let mut models = Vec::new();

        for (id, configured) in config.provider {
            if config.disabled_providers.contains(&id) {
                continue;
            }
            let compatible = configured.npm.as_deref() == Some("@ai-sdk/openai-compatible")
                || configured.npm.as_deref() == Some("@ai-sdk/openai")
                || configured.npm.is_none();
            if !compatible {
                continue;
            }
            let Some(base_url) = configured.options.base_url.clone() else {
                continue;
            };
            let credential = configured
                .options
                .api_key
                .clone()
                .or_else(|| match auth.get(&id) {
                    Some(AuthEntry::Api { key }) => Some(key.clone()),
                    Some(AuthEntry::Oauth { access, .. }) => Some(access.clone()),
                    _ => None,
                });
            let mut headers = HeaderMap::new();
            for (name, value) in &configured.options.headers {
                let name = HeaderName::from_bytes(name.as_bytes())
                    .with_context(|| format!("invalid header name for provider {id}"))?;
                let value = HeaderValue::from_str(value)
                    .with_context(|| format!("invalid header value for provider {id}"))?;
                headers.insert(name, value);
            }
            for (model_id, model) in &configured.models {
                models.push(ModelInfo {
                    id: format!("{id}/{model_id}"),
                    provider: id.clone(),
                    upstream_id: model_id.clone(),
                    name: model.name.clone().unwrap_or_else(|| model_id.clone()),
                    reasoning: model.reasoning,
                    context_window: model.limit.context.or(model.limit.input),
                    max_output_tokens: model.limit.output,
                });
            }
            providers.insert(
                id.clone(),
                Provider {
                    base_url,
                    credential,
                    headers,
                    models: configured.models,
                },
            );
        }

        models.sort_by(|a, b| a.id.cmp(&b.id));
        let client = Client::builder()
            .user_agent(concat!("crabcodex/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            inner: Arc::new(RegistryInner {
                client,
                providers,
                models,
            }),
        })
    }

    pub fn client(&self) -> &Client {
        &self.inner.client
    }
    pub fn models(&self) -> &[ModelInfo] {
        &self.inner.models
    }
    pub fn provider_count(&self) -> usize {
        self.inner.providers.len()
    }

    pub fn resolve(&self, model: &str) -> anyhow::Result<(&Provider, String)> {
        let (provider_id, upstream_id) = model
            .split_once('/')
            .context("model must use the provider/model format")?;
        let provider = self
            .inner
            .providers
            .get(provider_id)
            .with_context(|| format!("unknown or unsupported provider '{provider_id}'"))?;
        if !provider.models.contains_key(upstream_id) {
            bail!("unknown model '{model}'");
        }
        Ok((provider, upstream_id.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn loads_only_enabled_compatible_providers() {
        let dir = tempdir().unwrap();
        let config_path = dir.path().join("opencode.jsonc");
        let auth_path = dir.path().join("auth.json");
        fs::write(&config_path, r#"{
          disabled_providers: ["disabled"],
          provider: {
            demo: { npm: "@ai-sdk/openai-compatible", options: { baseURL: "https://example.test/v1" }, models: { m: { name: "M" } } },
            disabled: { options: { baseURL: "https://off.test/v1" }, models: { x: {} } },
            anthropic: { npm: "@ai-sdk/anthropic", options: { baseURL: "https://a.test" }, models: { c: {} } }
          }
        }"#).unwrap();
        fs::write(&auth_path, r#"{"demo":{"type":"api","key":"secret"}}"#).unwrap();
        let registry = Registry::load(&ConfigPaths {
            config: config_path,
            auth: auth_path,
        })
        .unwrap();
        assert_eq!(registry.provider_count(), 1);
        assert_eq!(registry.models()[0].id, "demo/m");
        assert_eq!(
            registry.resolve("demo/m").unwrap().0.credential.as_deref(),
            Some("secret")
        );
    }
}
