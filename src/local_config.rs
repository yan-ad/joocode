use std::{fs, path::PathBuf};

use crate::provider::WireApi;
use anyhow::{Context, bail};
use reqwest::{Client, Url, header};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalProvider {
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub api_keys: Vec<String>,
    pub models: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(
        default,
        alias = "wireApi",
        skip_serializing_if = "is_default_wire_api"
    )]
    pub wire_api: WireApi,
}

pub fn add_api_key(name: &str, api_key: &str) -> anyhow::Result<PathBuf> {
    let path = path()?;
    add_api_key_to(&path, name, api_key)?;
    Ok(path)
}

fn add_api_key_to(path: &std::path::Path, name: &str, api_key: &str) -> anyhow::Result<()> {
    let api_key = api_key.trim();
    if api_key.is_empty() {
        bail!("API key cannot be empty");
    }

    let mut providers = load_from(path)?;
    let provider = providers
        .iter_mut()
        .find(|provider| provider.name == name)
        .context("selected provider was not found")?;
    let mut keys = provider.api_keys();
    if !keys.iter().any(|key| key == api_key) {
        keys.push(api_key.to_owned());
    }
    provider.api_key.clear();
    provider.api_keys = keys;
    write_providers(path, &providers)
}

pub fn remove_last_api_key(name: &str) -> anyhow::Result<PathBuf> {
    let path = path()?;
    remove_last_api_key_from(&path, name)?;
    Ok(path)
}

fn remove_last_api_key_from(path: &std::path::Path, name: &str) -> anyhow::Result<()> {
    let mut providers = load_from(path)?;
    let provider = providers
        .iter_mut()
        .find(|provider| provider.name == name)
        .context("selected provider was not found")?;
    let mut keys = provider.api_keys();
    if keys.len() <= 1 {
        bail!("provider must retain at least one API key");
    }
    keys.pop();
    provider.api_key.clear();
    provider.api_keys = keys;
    write_providers(path, &providers)
}

fn is_default_wire_api(value: &WireApi) -> bool {
    *value == WireApi::OpenAiChat
}

pub fn set_default_model(name: &str, model: &str) -> anyhow::Result<PathBuf> {
    let path = path()?;
    let mut providers = load_from(&path)?;
    let selected = providers
        .iter()
        .find(|provider| provider.name == name)
        .context("selected provider was not found")?;
    if !selected.models.iter().any(|candidate| candidate == model) {
        bail!("model `{model}` does not belong to provider `{name}`");
    }
    for provider in &mut providers {
        provider.default_model = (provider.name == name).then(|| model.to_owned());
    }
    write_providers(&path, &providers)?;
    Ok(path)
}

pub fn default_model_route() -> anyhow::Result<Option<String>> {
    Ok(load()?.into_iter().find_map(|provider| {
        provider
            .default_model
            .map(|model| format!("joocode/{}/{model}", provider.name))
    }))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSummary {
    pub name: String,
    pub label: String,
    pub model_count: usize,
    pub models: Vec<String>,
    pub default_model: Option<String>,
    pub key_count: usize,
}

impl LocalProvider {
    pub fn api_keys(&self) -> Vec<String> {
        let mut keys = self
            .api_keys
            .iter()
            .map(|key| key.trim())
            .filter(|key| !key.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if !self.api_key.trim().is_empty() && !keys.iter().any(|key| key == self.api_key.trim()) {
            keys.insert(0, self.api_key.trim().to_owned());
        }
        keys
    }

    pub fn summary(&self) -> ProviderSummary {
        ProviderSummary {
            name: self.name.clone(),
            label: provider_label(&self.base_url).unwrap_or_else(|_| self.name.clone()),
            model_count: self.models.len(),
            models: self.models.clone(),
            default_model: self.default_model.clone(),
            key_count: self.api_keys().len(),
        }
    }
}

pub fn path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("JOOCODE_PROVIDERS") {
        return Ok(PathBuf::from(path));
    }
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .context("cannot determine Joocode config directory")?;
    Ok(root.join("joocode/providers.json"))
}

pub fn load() -> anyhow::Result<Vec<LocalProvider>> {
    load_from(&path()?)
}

pub fn summaries() -> anyhow::Result<Vec<ProviderSummary>> {
    Ok(load()?.iter().map(LocalProvider::summary).collect())
}

fn load_from(path: &std::path::Path) -> anyhow::Result<Vec<LocalProvider>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed reading {}", path.display()))?;
    let providers = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON in {}", path.display()))?;
    Ok(providers)
}

pub async fn probe(
    client: &Client,
    base_url: &str,
    api_key: &str,
) -> anyhow::Result<LocalProvider> {
    let base_url = normalize_base_url(base_url)?;
    let mut request = client.get(format!("{base_url}/models"));
    if !api_key.trim().is_empty() {
        request = request.header(header::AUTHORIZATION, format!("Bearer {}", api_key.trim()));
    }
    let response = request
        .send()
        .await
        .context("failed requesting the OpenAI-compatible /models endpoint")?
        .error_for_status()
        .context("the OpenAI-compatible /models endpoint rejected the request")?;
    let body: Value = response
        .json()
        .await
        .context("the OpenAI-compatible /models endpoint returned invalid JSON")?;
    let models = parse_models(&body);
    if models.is_empty() {
        bail!("the /models endpoint returned no model IDs");
    }
    Ok(LocalProvider {
        name: provider_name(&base_url)?,
        base_url,
        api_key: api_key.trim().to_owned(),
        api_keys: Vec::new(),
        models,
        default_model: None,
        wire_api: WireApi::OpenAiChat,
    })
}

pub fn save(provider: LocalProvider) -> anyhow::Result<PathBuf> {
    let path = path()?;
    let mut providers = load_from(&path)?;
    if let Some(existing) = providers
        .iter_mut()
        .find(|entry| entry.name == provider.name)
    {
        let default_model = existing
            .default_model
            .clone()
            .filter(|model| provider.models.contains(model));
        *existing = LocalProvider {
            default_model,
            ..provider
        };
    } else {
        providers.push(provider);
        providers.sort_by(|a, b| a.name.cmp(&b.name));
    }
    write_providers(&path, &providers)?;
    Ok(path)
}

pub fn remove(name: &str) -> anyhow::Result<PathBuf> {
    let path = path()?;
    remove_from(&path, name)?;
    Ok(path)
}

fn remove_from(path: &std::path::Path, name: &str) -> anyhow::Result<()> {
    let mut providers = load_from(path)?;
    let original_len = providers.len();
    providers.retain(|provider| provider.name != name);
    if providers.len() == original_len {
        bail!("provider `{name}` was not found");
    }
    write_providers(path, &providers)?;
    Ok(())
}

fn write_providers(path: &std::path::Path, providers: &[LocalProvider]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(providers)?)
        .with_context(|| format!("failed writing {}", temporary.display()))?;
    set_private_permissions(&temporary)?;
    fs::rename(&temporary, path).with_context(|| format!("failed replacing {}", path.display()))?;
    Ok(())
}

fn normalize_base_url(value: &str) -> anyhow::Result<String> {
    let value = value.trim().trim_end_matches('/');
    let url = Url::parse(value).context("base URL must be a valid http(s) URL")?;
    if !matches!(url.scheme(), "http" | "https") {
        bail!("base URL must use http or https");
    }
    Ok(value.to_owned())
}

fn provider_name(base_url: &str) -> anyhow::Result<String> {
    let url = Url::parse(base_url)?;
    let host = url.host_str().context("base URL has no host")?;
    let host = host
        .strip_prefix("api.")
        .or_else(|| host.strip_prefix("www."))
        .unwrap_or(host);
    let candidate = host.split('.').next().unwrap_or("custom");
    let mut name = candidate
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>();
    if let Some(port) = url.port()
        && !matches!((url.scheme(), port), ("http", 80) | ("https", 443))
    {
        name.push('-');
        name.push_str(&port.to_string());
    }
    if name.is_empty() {
        bail!("cannot derive a provider name from the base URL");
    }
    Ok(name)
}

fn provider_label(base_url: &str) -> anyhow::Result<String> {
    let url = Url::parse(base_url)?;
    let host = url.host_str().context("base URL has no host")?;
    let host = host
        .strip_prefix("api.")
        .or_else(|| host.strip_prefix("www."))
        .unwrap_or(host);
    Ok(match url.port() {
        Some(port) if !matches!((url.scheme(), port), ("http", 80) | ("https", 443)) => {
            format!("{host}:{port}")
        }
        _ => host.to_owned(),
    })
}

fn parse_models(value: &Value) -> Vec<String> {
    let entries = value
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| value.get("models").and_then(Value::as_array))
        .or_else(|| value.as_array());
    let mut models = entries
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            entry
                .as_str()
                .or_else(|| entry.get("id").and_then(Value::as_str))
                .or_else(|| entry.get("name").and_then(Value::as_str))
        })
        .filter(|id| !id.trim().is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}

#[cfg(unix)]
fn set_private_permissions(path: &std::path::Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &std::path::Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_models_responses() {
        assert_eq!(
            parse_models(&serde_json::json!({"data": [{"id": "b"}, {"id": "a"}]})),
            vec!["a", "b"]
        );
        assert_eq!(
            parse_models(&serde_json::json!({"models": ["x", {"name": "y"}]})),
            vec!["x", "y"]
        );
    }

    #[test]
    fn derives_stable_provider_name() {
        assert_eq!(
            provider_name("https://api.openrouter.ai/v1").unwrap(),
            "openrouter"
        );
        assert_eq!(
            provider_name("http://localhost:11434/v1").unwrap(),
            "localhost-11434"
        );
        assert_eq!(
            provider_label("https://api.openai.com/v1").unwrap(),
            "openai.com"
        );
        assert_eq!(
            provider_label("https://gunamaya.id/v1").unwrap(),
            "gunamaya.id"
        );
    }

    #[test]
    fn saves_flat_provider_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("providers.json");
        let providers = vec![LocalProvider {
            name: "local".into(),
            base_url: "http://localhost:1234/v1".into(),
            api_key: "secret".into(),
            api_keys: Vec::new(),
            models: vec!["model-a".into()],
            default_model: None,
            wire_api: WireApi::OpenAiChat,
        }];
        fs::write(&path, serde_json::to_vec_pretty(&providers).unwrap()).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].models, vec!["model-a"]);
    }

    #[test]
    fn accepts_legacy_api_key_and_new_key_pool() {
        let legacy: LocalProvider = serde_json::from_value(serde_json::json!({
            "name":"legacy", "base_url":"https://example.test/v1", "api_key":"one", "models":["m"]
        }))
        .unwrap();
        assert_eq!(legacy.api_keys(), vec!["one"]);

        let pooled: LocalProvider = serde_json::from_value(serde_json::json!({
            "name":"pool", "base_url":"https://example.test/v1", "api_keys":["secret-alpha", "secret-beta"], "models":["m"]
        })).unwrap();
        assert_eq!(pooled.api_keys(), vec!["secret-alpha", "secret-beta"]);
        let summary = format!("{:?}", pooled.summary());
        assert!(!summary.contains("secret-alpha") && !summary.contains("secret-beta"));
    }

    #[test]
    fn wire_api_is_backward_compatible_and_accepts_responses() {
        let legacy: LocalProvider = serde_json::from_value(serde_json::json!({
            "name":"legacy", "base_url":"https://example.test/v1", "models":["m"]
        }))
        .unwrap();
        assert_eq!(legacy.wire_api, WireApi::OpenAiChat);
        let native: LocalProvider = serde_json::from_value(serde_json::json!({
            "name":"native", "base_url":"https://example.test/v1", "models":["m"],
            "wire_api":"open_ai_responses"
        }))
        .unwrap();
        assert_eq!(native.wire_api, WireApi::OpenAiResponses);
    }

    #[test]
    fn removes_provider_without_exposing_other_entries() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("providers.json");
        let providers = vec![
            LocalProvider {
                name: "gunamaya".into(),
                base_url: "https://gunamaya.id/v1".into(),
                api_key: "secret-a".into(),
                api_keys: Vec::new(),
                models: vec!["model-a".into()],
                default_model: None,
                wire_api: WireApi::OpenAiChat,
            },
            LocalProvider {
                name: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: "secret-b".into(),
                api_keys: Vec::new(),
                models: vec!["model-b".into()],
                default_model: None,
                wire_api: WireApi::OpenAiChat,
            },
        ];
        write_providers(&path, &providers).unwrap();
        remove_from(&path, "gunamaya").unwrap();
        let remaining = load_from(&path).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "openai");
        assert_eq!(remaining[0].api_key, "secret-b");
    }

    #[test]
    fn sets_one_global_default_model() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("providers.json");
        let providers = vec![
            LocalProvider {
                name: "gunamaya".into(),
                base_url: "https://gunamaya.id/v1".into(),
                api_key: "secret-a".into(),
                api_keys: Vec::new(),
                models: vec!["gpt-5.5".into()],
                default_model: None,
                wire_api: WireApi::OpenAiChat,
            },
            LocalProvider {
                name: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: "secret-b".into(),
                api_keys: Vec::new(),
                models: vec!["gpt-5.4".into()],
                default_model: Some("gpt-5.4".into()),
                wire_api: WireApi::OpenAiChat,
            },
        ];
        write_providers(&path, &providers).unwrap();

        let mut loaded = load_from(&path).unwrap();
        let selected = loaded
            .iter()
            .find(|provider| provider.name == "gunamaya")
            .unwrap();
        assert!(selected.models.contains(&"gpt-5.5".into()));
        for provider in &mut loaded {
            provider.default_model = (provider.name == "gunamaya").then(|| "gpt-5.5".to_owned());
        }
        write_providers(&path, &loaded).unwrap();

        let result = load_from(&path).unwrap();
        assert_eq!(result[0].default_model.as_deref(), Some("gpt-5.5"));
        assert_eq!(result[1].default_model, None);
    }

    #[test]
    fn manages_key_pools_without_exposing_secret_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("providers.json");
        write_providers(
            &path,
            &[LocalProvider {
                name: "demo".into(),
                base_url: "https://example.test/v1".into(),
                api_key: "first".into(),
                api_keys: Vec::new(),
                models: vec!["model".into()],
                default_model: None,
                wire_api: WireApi::OpenAiChat,
            }],
        )
        .unwrap();
        add_api_key_to(&path, "demo", "second").unwrap();
        let provider = load_from(&path).unwrap().remove(0);
        assert_eq!(provider.api_keys(), ["first", "second"]);
        let summary = provider.summary();
        assert_eq!(summary.key_count, 2);
        assert!(!format!("{summary:?}").contains("first"));
        assert!(!format!("{summary:?}").contains("second"));
        remove_last_api_key_from(&path, "demo").unwrap();
        assert_eq!(load_from(&path).unwrap()[0].api_keys(), ["first"]);
        assert!(remove_last_api_key_from(&path, "demo").is_err());
    }

    #[tokio::test]
    async fn probes_models_with_bearer_auth() {
        use axum::{Json, Router, http::HeaderMap, routing::get};

        let app = Router::new().route(
            "/v1/models",
            get(|headers: HeaderMap| async move {
                assert_eq!(
                    headers.get(header::AUTHORIZATION).unwrap(),
                    "Bearer local-key"
                );
                Json(serde_json::json!({"data": [{"id": "model-b"}, {"id": "model-a"}]}))
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let provider = probe(&Client::new(), &format!("http://{address}/v1"), "local-key")
            .await
            .unwrap();
        assert_eq!(provider.models, vec!["model-a", "model-b"]);
        assert!(provider.name.starts_with("127-"));
    }
}
