use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, bail};
use clap::ValueEnum;
use reqwest::{
    Client,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde_json::Value;

use crate::{
    config::{self, AuthEntry, ConfigPaths, ModelConfig},
    provider::{CopilotCredential, Credential, ModelInfo, Provider},
};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, ValueEnum)]
pub enum SourceKind {
    Auto,
    #[value(name = "opencode")]
    OpenCode,
    Ocx,
    Hermes,
    Copilot,
}

fn source_error(source: &str, error: anyhow::Error) -> anyhow::Error {
    error.context(format!("source:{source}"))
}

#[derive(Clone, Debug)]
pub struct SourceSelection {
    enabled: BTreeSet<SourceKind>,
    pub opencode_config: Option<PathBuf>,
    pub opencode_auth: Option<PathBuf>,
}

impl SourceSelection {
    pub fn new(
        requested: Vec<SourceKind>,
        opencode_config: Option<PathBuf>,
        opencode_auth: Option<PathBuf>,
    ) -> anyhow::Result<Self> {
        let mut enabled = requested.into_iter().collect::<BTreeSet<_>>();
        if enabled.is_empty() || enabled.remove(&SourceKind::Auto) {
            enabled.extend([
                SourceKind::OpenCode,
                SourceKind::Ocx,
                SourceKind::Hermes,
                SourceKind::Copilot,
            ]);
        }
        if enabled.is_empty() {
            bail!("select at least one provider source");
        }
        Ok(Self {
            enabled,
            opencode_config,
            opencode_auth,
        })
    }

    fn enabled(&self, source: SourceKind) -> bool {
        self.enabled.contains(&source)
    }
}

#[derive(Clone, Debug)]
pub struct DiscoveredModel {
    pub info: ModelInfo,
}

#[derive(Clone, Debug)]
pub struct DiscoveredProvider {
    pub key: String,
    pub provider: Provider,
    pub models: Vec<DiscoveredModel>,
}

#[derive(Clone, Debug)]
pub struct DiscoveredCatalog {
    pub source: String,
    pub providers: Vec<DiscoveredProvider>,
    pub detail: Option<String>,
}

pub async fn discover(
    selection: &SourceSelection,
    client: &Client,
) -> Vec<anyhow::Result<DiscoveredCatalog>> {
    let mut catalogs = Vec::new();
    if selection.enabled(SourceKind::OpenCode) {
        catalogs
            .push(discover_opencode(selection).map_err(|error| source_error("opencode", error)));
    }
    if selection.enabled(SourceKind::Ocx) {
        catalogs.push(discover_ocx(selection).map_err(|error| source_error("ocx", error)));
    }
    if selection.enabled(SourceKind::Hermes) {
        catalogs.push(discover_hermes().map_err(|error| source_error("hermes", error)));
    }
    if selection.enabled(SourceKind::Copilot) {
        catalogs.push(
            discover_copilot(client)
                .await
                .map_err(|error| source_error("copilot", error)),
        );
    }
    catalogs
}

fn empty_catalog(source: &str, detail: impl Into<String>) -> DiscoveredCatalog {
    DiscoveredCatalog {
        source: source.to_owned(),
        providers: Vec::new(),
        detail: Some(detail.into()),
    }
}

fn discover_opencode(selection: &SourceSelection) -> anyhow::Result<DiscoveredCatalog> {
    let Some(paths) = ConfigPaths::discover(
        selection.opencode_config.clone(),
        selection.opencode_auth.clone(),
    )?
    else {
        return Ok(empty_catalog("opencode", "config not found"));
    };
    load_opencode_catalog("opencode", None, &paths)
}

fn discover_ocx(selection: &SourceSelection) -> anyhow::Result<DiscoveredCatalog> {
    let profiles_root = xdg_config_home()?.join("opencode/profiles");
    if !profiles_root.is_dir() {
        return Ok(empty_catalog("ocx", "profiles not found"));
    }
    let auth = resolve_opencode_auth(selection.opencode_auth.clone())?;
    if !auth.is_file() {
        return Ok(empty_catalog("ocx", "OpenCode auth not found"));
    }

    discover_ocx_at(&profiles_root, &auth)
}

fn discover_ocx_at(profiles_root: &Path, auth: &Path) -> anyhow::Result<DiscoveredCatalog> {
    let mut providers = Vec::new();
    let mut profile_count = 0;
    for entry in fs::read_dir(profiles_root)
        .with_context(|| format!("failed reading {}", profiles_root.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let profile = entry.file_name().to_string_lossy().into_owned();
        let config = entry.path().join("opencode.jsonc");
        if !config.is_file() {
            continue;
        }
        profile_count += 1;
        let paths = ConfigPaths {
            config,
            auth: auth.to_owned(),
        };
        let namespace = format!("ocx-{profile}");
        let catalog = load_opencode_catalog("ocx", Some(&namespace), &paths)
            .with_context(|| format!("invalid OCX profile '{profile}'"))?;
        providers.extend(catalog.providers);
    }
    Ok(DiscoveredCatalog {
        source: "ocx".into(),
        providers,
        detail: Some(format!("{profile_count} profiles scanned")),
    })
}

pub(crate) fn load_opencode_catalog(
    source: &str,
    namespace: Option<&str>,
    paths: &ConfigPaths,
) -> anyhow::Result<DiscoveredCatalog> {
    let (loaded, auth) = config::load(paths)?;
    let mut providers = Vec::new();
    for (id, configured) in loaded.provider {
        if loaded.disabled_providers.contains(&id) {
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
            })
            .map(Credential::Bearer)
            .unwrap_or(Credential::None);
        let headers = header_map(&configured.options.headers, &id)?;
        let public_provider = namespace
            .map(|prefix| format!("{prefix}/{id}"))
            .unwrap_or_else(|| id.clone());
        let models = configured
            .models
            .iter()
            .map(|(model_id, model)| discovered_model(&public_provider, model_id, model_id, model))
            .collect();
        providers.push(DiscoveredProvider {
            key: format!("{source}:{public_provider}"),
            provider: Provider {
                base_url,
                credential,
                headers,
            },
            models,
        });
    }
    Ok(DiscoveredCatalog {
        source: source.into(),
        providers,
        detail: Some(paths.config.display().to_string()),
    })
}

fn discover_hermes() -> anyhow::Result<DiscoveredCatalog> {
    let home = env::var_os("HERMES_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".hermes"));
    discover_hermes_at(&home)
}

fn discover_hermes_at(home: &Path) -> anyhow::Result<DiscoveredCatalog> {
    let config_path = home.join("config.yaml");
    if !config_path.is_file() {
        return Ok(empty_catalog("hermes", "config not found"));
    }
    let text = fs::read_to_string(&config_path)
        .with_context(|| format!("failed reading {}", config_path.display()))?;
    let root: serde_yaml::Value = serde_yaml::from_str(&text)
        .with_context(|| format!("invalid YAML in {}", config_path.display()))?;
    let env_file = parse_dotenv(&home.join(".env"))?;
    let mut providers = Vec::new();

    if let Some(entries) = yaml_mapping(root.get("providers")) {
        for (key, value) in entries {
            let Some(id) = key.as_str() else { continue };
            if value.get("enabled").and_then(serde_yaml::Value::as_bool) == Some(false) {
                continue;
            }
            if let Some(provider) = hermes_provider(id, value, &env_file)? {
                providers.push(provider);
            }
        }
    }

    // Legacy Hermes configs used a list instead of the modern providers map.
    if let Some(entries) = root
        .get("custom_providers")
        .and_then(serde_yaml::Value::as_sequence)
    {
        for value in entries {
            let Some(id) = value.get("name").and_then(serde_yaml::Value::as_str) else {
                continue;
            };
            if let Some(provider) = hermes_provider(id, value, &env_file)? {
                providers.push(provider);
            }
        }
    }

    if let Some(model) = root.get("model")
        && let Some(base_url) = yaml_string(model, &["base_url", "baseUrl"])
    {
        let configured_provider = model
            .get("provider")
            .and_then(serde_yaml::Value::as_str)
            .unwrap_or("auto");
        let id = if configured_provider == "auto" {
            infer_hermes_provider(&base_url).unwrap_or("main")
        } else {
            configured_provider
        };
        let model_id =
            yaml_string(model, &["default", "model"]).unwrap_or_else(|| "default".into());
        let credential = resolve_hermes_credential(model, id, &env_file);
        providers.push(DiscoveredProvider {
            key: format!("hermes:{id}"),
            provider: Provider {
                base_url,
                credential,
                headers: hermes_headers(model, id, &env_file)?,
            },
            models: vec![simple_model(
                &format!("hermes/{id}"),
                &model_id,
                &model_id,
                model
                    .get("context_length")
                    .and_then(serde_yaml::Value::as_u64),
                model.get("max_tokens").and_then(serde_yaml::Value::as_u64),
            )],
        });
    }

    Ok(DiscoveredCatalog {
        source: "hermes".into(),
        providers,
        detail: Some(config_path.display().to_string()),
    })
}

fn hermes_provider(
    id: &str,
    value: &serde_yaml::Value,
    env_file: &BTreeMap<String, String>,
) -> anyhow::Result<Option<DiscoveredProvider>> {
    let Some(base_url) = yaml_string(value, &["base_url", "baseUrl", "url", "api"]) else {
        return Ok(None);
    };
    let transport =
        yaml_string(value, &["api_mode", "transport"]).unwrap_or_else(|| "chat_completions".into());
    if !matches!(
        transport.as_str(),
        "chat_completions" | "openai_chat" | "completions"
    ) {
        return Ok(None);
    }
    let mut model_ids = yaml_model_ids(value.get("models"));
    if model_ids.is_empty()
        && let Some(model) = yaml_string(value, &["model", "default_model", "defaultModel"])
    {
        model_ids.push(model);
    }
    if model_ids.is_empty() {
        return Ok(None);
    }
    let context = value
        .get("context_length")
        .and_then(serde_yaml::Value::as_u64);
    let public_provider = format!("hermes/{id}");
    let models = model_ids
        .iter()
        .map(|model_id| simple_model(&public_provider, model_id, model_id, context, None))
        .collect();
    Ok(Some(DiscoveredProvider {
        key: format!("hermes:{id}"),
        provider: Provider {
            base_url,
            credential: resolve_hermes_credential(value, id, env_file),
            headers: hermes_headers(value, id, env_file)?,
        },
        models,
    }))
}

async fn discover_copilot(client: &Client) -> anyhow::Result<DiscoveredCatalog> {
    let Some(raw_token) = copilot_raw_token() else {
        return Ok(empty_catalog("copilot", "GitHub token not found"));
    };
    let credential = CopilotCredential::new(raw_token);
    let (api_token, base_url) = credential
        .exchange(client)
        .await
        .context("copilot: token exchange failed; run `copilot login`")?;
    let base_url = base_url.unwrap_or_else(|| "https://api.githubcopilot.com".into());
    let headers = copilot_headers()?;
    let mut request_headers = headers.clone();
    request_headers.insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {api_token}"))?,
    );
    let payload: Value = client
        .get(format!("{}/models", base_url.trim_end_matches('/')))
        .headers(request_headers)
        .send()
        .await
        .context("copilot: model discovery failed")?
        .error_for_status()
        .context("copilot: model catalog was rejected")?
        .json()
        .await
        .context("copilot: invalid model catalog")?;
    let items = payload
        .get("data")
        .or_else(|| payload.get("models"))
        .and_then(Value::as_array)
        .or_else(|| payload.as_array())
        .context("copilot: model catalog contains no model list")?;
    let mut models = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(Value::as_str) else {
            continue;
        };
        if let Some(endpoints) = item.get("supported_endpoints").and_then(Value::as_array)
            && !endpoints
                .iter()
                .filter_map(Value::as_str)
                .any(|endpoint| endpoint.ends_with("/chat/completions"))
        {
            continue;
        }
        let context = item
            .pointer("/capabilities/limits/max_prompt_tokens")
            .and_then(Value::as_u64);
        let output = item
            .pointer("/capabilities/limits/max_output_tokens")
            .and_then(Value::as_u64);
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(id)
            .to_owned();
        models.push(DiscoveredModel {
            info: ModelInfo {
                id: format!("copilot/{id}"),
                provider: "copilot".into(),
                upstream_id: id.into(),
                name,
                reasoning: item
                    .pointer("/capabilities/supports/reasoning_effort")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                context_window: context,
                max_output_tokens: output,
            },
        });
    }
    Ok(DiscoveredCatalog {
        source: "copilot".into(),
        providers: vec![DiscoveredProvider {
            key: "copilot".into(),
            provider: Provider {
                base_url,
                credential: Credential::Copilot(credential),
                headers,
            },
            models,
        }],
        detail: Some("GitHub Copilot live catalog".into()),
    })
}

fn copilot_raw_token() -> Option<String> {
    for name in ["COPILOT_GITHUB_TOKEN", "GH_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(value) = env::var(name)
            && copilot_token_is_supported(value.trim())
        {
            return Some(value.trim().to_owned());
        }
    }

    #[cfg(target_os = "macos")]
    for (service, account) in [
        ("copilot-cli", None),
        ("copilot-language-server", Some("oauth-token-key")),
    ] {
        let mut command = Command::new("security");
        command.args(["find-generic-password", "-s", service]);
        if let Some(account) = account {
            command.args(["-a", account]);
        }
        if let Ok(output) = command.arg("-w").output()
            && output.status.success()
            && let Ok(token) = String::from_utf8(output.stdout)
            && copilot_token_is_supported(token.trim())
        {
            return Some(token.trim().to_owned());
        }
    }

    if let Some(token) = copilot_plaintext_token() {
        return Some(token);
    }

    let output = Command::new("gh").args(["auth", "token"]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let token = String::from_utf8(output.stdout).ok()?;
    copilot_token_is_supported(token.trim()).then(|| token.trim().to_owned())
}

fn copilot_token_is_supported(token: &str) -> bool {
    !token.is_empty() && !token.starts_with("ghp_")
}

fn copilot_plaintext_token() -> Option<String> {
    let home = env::var_os("COPILOT_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".copilot"));
    let text = fs::read_to_string(home.join("config.json")).ok()?;
    let value: Value = json5::from_str(&text).ok()?;
    find_copilot_token(&value)
}

fn find_copilot_token(value: &Value) -> Option<String> {
    match value {
        Value::String(value)
            if value.starts_with("gho_")
                || value.starts_with("ghu_")
                || value.starts_with("github_pat_") =>
        {
            Some(value.clone())
        }
        Value::Array(values) => values.iter().find_map(find_copilot_token),
        Value::Object(values) => values.values().find_map(find_copilot_token),
        _ => None,
    }
}

fn copilot_headers() -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in [
        ("editor-version", "vscode/1.104.1"),
        ("user-agent", "Joocode/1.0"),
        ("copilot-integration-id", "vscode-chat"),
        ("openai-intent", "conversation-edits"),
        ("x-initiator", "agent"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    Ok(headers)
}

fn discovered_model(
    public_provider: &str,
    public_model: &str,
    upstream_model: &str,
    model: &ModelConfig,
) -> DiscoveredModel {
    DiscoveredModel {
        info: ModelInfo {
            id: format!("{public_provider}/{public_model}"),
            provider: public_provider.into(),
            upstream_id: upstream_model.into(),
            name: model.name.clone().unwrap_or_else(|| public_model.into()),
            reasoning: model.reasoning,
            context_window: model.limit.context.or(model.limit.input),
            max_output_tokens: model.limit.output,
        },
    }
}

fn simple_model(
    public_provider: &str,
    public_model: &str,
    upstream_model: &str,
    context_window: Option<u64>,
    max_output_tokens: Option<u64>,
) -> DiscoveredModel {
    DiscoveredModel {
        info: ModelInfo {
            id: format!("{public_provider}/{public_model}"),
            provider: public_provider.into(),
            upstream_id: upstream_model.into(),
            name: public_model.into(),
            reasoning: false,
            context_window,
            max_output_tokens,
        },
    }
}

fn header_map(values: &BTreeMap<String, String>, provider: &str) -> anyhow::Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    for (name, value) in values {
        let name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid header name for provider {provider}"))?;
        let value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid header value for provider {provider}"))?;
        headers.insert(name, value);
    }
    Ok(headers)
}

fn hermes_headers(
    value: &serde_yaml::Value,
    provider: &str,
    env_file: &BTreeMap<String, String>,
) -> anyhow::Result<HeaderMap> {
    let mut values = BTreeMap::new();
    for field in ["default_headers", "extra_headers"] {
        if let Some(mapping) = value.get(field).and_then(serde_yaml::Value::as_mapping) {
            for (name, value) in mapping {
                if let (Some(name), Some(value)) = (name.as_str(), value.as_str()) {
                    values.insert(name.to_owned(), expand_env_with(value, env_file));
                }
            }
        }
    }
    header_map(&values, provider)
}

fn resolve_hermes_credential(
    value: &serde_yaml::Value,
    provider: &str,
    env_file: &BTreeMap<String, String>,
) -> Credential {
    if let Some(value) = value.get("api_key").and_then(serde_yaml::Value::as_str) {
        let expanded = expand_env_with(value, env_file);
        if !expanded.is_empty() {
            return Credential::Bearer(expanded);
        }
    }
    if let Some(name) = yaml_string(value, &["key_env", "api_key_env", "keyEnv"])
        && let Some(secret) = env::var(&name)
            .ok()
            .or_else(|| env_file.get(&name).cloned())
            .filter(|secret| !secret.is_empty())
    {
        return Credential::Bearer(secret);
    }
    for name in hermes_default_key_envs(provider) {
        if let Some(secret) = env::var(name)
            .ok()
            .or_else(|| env_file.get(*name).cloned())
            .filter(|secret| !secret.is_empty())
        {
            return Credential::Bearer(secret);
        }
    }
    Credential::None
}

fn hermes_default_key_envs(provider: &str) -> &'static [&'static str] {
    match provider {
        "openrouter" => &["OPENROUTER_API_KEY", "OPENAI_API_KEY"],
        "openai" | "openai-api" => &["OPENAI_API_KEY"],
        "lmstudio" => &["LM_API_KEY"],
        "zai" => &["GLM_API_KEY", "ZAI_API_KEY", "Z_AI_API_KEY"],
        "kimi-coding" => &["KIMI_API_KEY", "KIMI_CODING_API_KEY"],
        "minimax" => &["MINIMAX_API_KEY"],
        "deepinfra" => &["DEEPINFRA_API_KEY"],
        "nvidia" => &["NVIDIA_API_KEY"],
        "kilocode" => &["KILOCODE_API_KEY"],
        "ai-gateway" => &["AI_GATEWAY_API_KEY"],
        _ => &[],
    }
}

fn infer_hermes_provider(base_url: &str) -> Option<&'static str> {
    let normalized = base_url.to_ascii_lowercase();
    if normalized.contains("openrouter.ai") {
        Some("openrouter")
    } else if normalized.contains("api.openai.com") {
        Some("openai")
    } else if normalized.contains("api.z.ai") || normalized.contains("bigmodel.cn") {
        Some("zai")
    } else if normalized.contains("deepinfra.com") {
        Some("deepinfra")
    } else if normalized.contains("integrate.api.nvidia.com") {
        Some("nvidia")
    } else {
        None
    }
}

fn yaml_mapping(value: Option<&serde_yaml::Value>) -> Option<&serde_yaml::Mapping> {
    value.and_then(serde_yaml::Value::as_mapping)
}

fn yaml_string(value: &serde_yaml::Value, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| {
        value
            .get(*field)
            .and_then(serde_yaml::Value::as_str)
            .map(expand_env)
            .filter(|value| !value.is_empty())
    })
}

fn yaml_model_ids(value: Option<&serde_yaml::Value>) -> Vec<String> {
    let mut ids = Vec::new();
    match value {
        Some(serde_yaml::Value::Mapping(mapping)) => {
            ids.extend(
                mapping
                    .keys()
                    .filter_map(serde_yaml::Value::as_str)
                    .map(str::to_owned),
            );
        }
        Some(serde_yaml::Value::Sequence(sequence)) => {
            for item in sequence {
                if let Some(id) = item
                    .as_str()
                    .or_else(|| item.get("id").and_then(serde_yaml::Value::as_str))
                {
                    ids.push(id.to_owned());
                }
            }
        }
        _ => {}
    }
    ids
}

fn parse_dotenv(path: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    if !path.is_file() {
        return Ok(BTreeMap::new());
    }
    let text =
        fs::read_to_string(path).with_context(|| format!("failed reading {}", path.display()))?;
    let mut values = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().trim_matches(['\'', '"']);
        values.insert(name.trim().to_owned(), value.to_owned());
    }
    Ok(values)
}

fn expand_env(value: &str) -> String {
    expand_env_with(value, &BTreeMap::new())
}

fn expand_env_with(value: &str, fallback: &BTreeMap<String, String>) -> String {
    let trimmed = value.trim();
    let Some(name) = trimmed
        .strip_prefix("${")
        .and_then(|value| value.strip_suffix('}'))
    else {
        return trimmed.to_owned();
    };
    env::var(name)
        .ok()
        .or_else(|| fallback.get(name).cloned())
        .unwrap_or_default()
}

fn xdg_config_home() -> anyhow::Result<PathBuf> {
    Ok(env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or(
        dirs::home_dir()
            .context("could not resolve home directory")?
            .join(".config"),
    ))
}

fn resolve_opencode_auth(override_path: Option<PathBuf>) -> anyhow::Result<PathBuf> {
    if let Some(path) = override_path {
        return Ok(path);
    }
    for name in [
        "JOOCODE_AUTH",
        "JOC_AUTH",
        "CRABCODEX_AUTH",
        "OPEN_INITIATIVE_AUTH",
    ] {
        if let Some(path) = env::var_os(name).filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(path));
        }
    }
    Ok(env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or(
            dirs::home_dir()
                .context("could not resolve home directory")?
                .join(".local/share"),
        )
        .join("opencode/auth.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_hermes_provider_models_and_dotenv_secret() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("config.yaml"),
            r#"
providers:
  local:
    base_url: https://example.test/v1
    key_env: LOCAL_KEY
    models:
      fast: {context_length: 1000}
      smart: {}
"#,
        )
        .unwrap();
        fs::write(dir.path().join(".env"), "LOCAL_KEY=secret\n").unwrap();
        let catalog = discover_hermes_at(dir.path()).unwrap();
        assert_eq!(catalog.providers.len(), 1);
        assert_eq!(catalog.providers[0].models.len(), 2);
        assert_eq!(catalog.providers[0].models[0].info.id, "hermes/local/fast");
        assert!(matches!(
            catalog.providers[0].provider.credential,
            Credential::Bearer(_)
        ));
    }

    #[test]
    fn auto_selection_enables_every_source() {
        let selection = SourceSelection::new(vec![SourceKind::Auto], None, None).unwrap();
        assert!(selection.enabled(SourceKind::OpenCode));
        assert!(selection.enabled(SourceKind::Ocx));
        assert!(selection.enabled(SourceKind::Hermes));
        assert!(selection.enabled(SourceKind::Copilot));
    }

    #[test]
    fn discovers_namespaced_ocx_profiles() {
        let dir = tempdir().unwrap();
        let profiles = dir.path().join("profiles");
        let profile = profiles.join("work");
        let auth = dir.path().join("auth.json");
        fs::create_dir_all(&profile).unwrap();
        fs::write(
            profile.join("opencode.jsonc"),
            r#"{ provider: { demo: { options: { baseURL: "https://example.test/v1" }, models: { fast: {} } } } }"#,
        )
        .unwrap();
        fs::write(&auth, "{}").unwrap();

        let catalog = discover_ocx_at(&profiles, &auth).unwrap();
        assert_eq!(catalog.providers.len(), 1);
        assert_eq!(catalog.providers[0].models[0].info.id, "ocx-work/demo/fast");
    }

    #[test]
    fn infers_common_hermes_provider_from_base_url() {
        assert_eq!(
            infer_hermes_provider("https://openrouter.ai/api/v1"),
            Some("openrouter")
        );
        assert_eq!(infer_hermes_provider("http://localhost:11434/v1"), None);
    }
}
