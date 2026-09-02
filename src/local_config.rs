use std::{fs, path::PathBuf};

use anyhow::{Context, bail};
use reqwest::{Client, Url, header};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LocalProvider {
    pub name: String,
    pub base_url: String,
    pub api_key: String,
    pub models: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProviderSummary {
    pub name: String,
    pub label: String,
    pub model_count: usize,
}

impl LocalProvider {
    pub fn summary(&self) -> ProviderSummary {
        ProviderSummary {
            name: self.name.clone(),
            label: provider_label(&self.base_url).unwrap_or_else(|_| self.name.clone()),
            model_count: self.models.len(),
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
        models,
    })
}

pub fn save(provider: LocalProvider) -> anyhow::Result<PathBuf> {
    let path = path()?;
    let mut providers = load_from(&path)?;
    if let Some(existing) = providers
        .iter_mut()
        .find(|entry| entry.name == provider.name)
    {
        *existing = provider;
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
            models: vec!["model-a".into()],
        }];
        fs::write(&path, serde_json::to_vec_pretty(&providers).unwrap()).unwrap();
        let loaded = load_from(&path).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].models, vec!["model-a"]);
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
                models: vec!["model-a".into()],
            },
            LocalProvider {
                name: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                api_key: "secret-b".into(),
                models: vec!["model-b".into()],
            },
        ];
        write_providers(&path, &providers).unwrap();
        remove_from(&path, "gunamaya").unwrap();
        let remaining = load_from(&path).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].name, "openai");
        assert_eq!(remaining[0].api_key, "secret-b");
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
