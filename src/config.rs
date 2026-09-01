use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    path::PathBuf,
};

use anyhow::{Context, bail};
use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct ConfigPaths {
    pub config: PathBuf,
    pub auth: PathBuf,
}

fn xdg_dir(variable: &str, fallback: &str) -> anyhow::Result<PathBuf> {
    if let Some(path) = env::var_os(variable).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    Ok(dirs::home_dir()
        .context("could not resolve the user home directory")?
        .join(fallback))
}

impl ConfigPaths {
    pub fn resolve(config: Option<PathBuf>, auth: Option<PathBuf>) -> anyhow::Result<Self> {
        let config = match config {
            Some(path) => path,
            None if env::var_os("JOOCODE_CONFIG").is_some() => {
                PathBuf::from(env::var_os("JOOCODE_CONFIG").unwrap())
            }
            // Kept for upgrades from Joc and CrabCodex; JOOCODE_CONFIG takes precedence.
            None if env::var_os("JOC_CONFIG").is_some() => {
                PathBuf::from(env::var_os("JOC_CONFIG").unwrap())
            }
            None if env::var_os("CRABCODEX_CONFIG").is_some() => {
                PathBuf::from(env::var_os("CRABCODEX_CONFIG").unwrap())
            }
            None if env::var_os("OPEN_INITIATIVE_CONFIG").is_some() => {
                PathBuf::from(env::var_os("OPEN_INITIATIVE_CONFIG").unwrap())
            }
            None => xdg_dir("XDG_CONFIG_HOME", ".config")?.join("opencode/opencode.jsonc"),
        };
        let auth = match auth {
            Some(path) => path,
            None if env::var_os("JOOCODE_AUTH").is_some() => {
                PathBuf::from(env::var_os("JOOCODE_AUTH").unwrap())
            }
            // Kept for upgrades from Joc and CrabCodex; JOOCODE_AUTH takes precedence.
            None if env::var_os("JOC_AUTH").is_some() => {
                PathBuf::from(env::var_os("JOC_AUTH").unwrap())
            }
            None if env::var_os("CRABCODEX_AUTH").is_some() => {
                PathBuf::from(env::var_os("CRABCODEX_AUTH").unwrap())
            }
            None if env::var_os("OPEN_INITIATIVE_AUTH").is_some() => {
                PathBuf::from(env::var_os("OPEN_INITIATIVE_AUTH").unwrap())
            }
            None => xdg_dir("XDG_DATA_HOME", ".local/share")?.join("opencode/auth.json"),
        };
        if !config.is_file() {
            bail!("OpenCode config not found at {}", config.display());
        }
        if !auth.is_file() {
            bail!("OpenCode auth not found at {}", auth.display());
        }
        Ok(Self { config, auth })
    }

    pub fn discover(
        config: Option<PathBuf>,
        auth: Option<PathBuf>,
    ) -> anyhow::Result<Option<Self>> {
        let explicit = config.is_some() || auth.is_some();
        match Self::resolve(config, auth) {
            Ok(paths) => Ok(Some(paths)),
            Err(_error) if !explicit => Ok(None),
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenCodeConfig {
    #[serde(default, alias = "disabledProviders")]
    pub disabled_providers: BTreeSet<String>,
    #[serde(default)]
    pub provider: BTreeMap<String, ProviderConfig>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub npm: Option<String>,
    #[serde(default)]
    pub options: ProviderOptions,
    #[serde(default)]
    pub models: BTreeMap<String, ModelConfig>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ProviderOptions {
    #[serde(default, alias = "baseUrl", alias = "baseURL")]
    pub base_url: Option<String>,
    #[serde(default, alias = "apiKey")]
    pub api_key: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub limit: ModelLimit,
    #[serde(default, rename = "modalities")]
    pub _modalities: Value,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ModelLimit {
    #[serde(default)]
    pub input: Option<u64>,
    #[serde(default)]
    pub output: Option<u64>,
    #[serde(default)]
    pub context: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum AuthEntry {
    Api {
        key: String,
    },
    Oauth {
        access: String,
        #[serde(default)]
        _refresh: Option<String>,
        #[serde(default)]
        _expires: Option<u64>,
        #[serde(default, rename = "accountId")]
        _account_id: Option<String>,
    },
    #[serde(other)]
    Unknown,
}

pub type AuthConfig = BTreeMap<String, AuthEntry>;

pub fn load(paths: &ConfigPaths) -> anyhow::Result<(OpenCodeConfig, AuthConfig)> {
    let config_text = fs::read_to_string(&paths.config)
        .with_context(|| format!("failed reading {}", paths.config.display()))?;
    let auth_text = fs::read_to_string(&paths.auth)
        .with_context(|| format!("failed reading {}", paths.auth.display()))?;
    let config = json5::from_str(&config_text)
        .with_context(|| format!("invalid JSONC in {}", paths.config.display()))?;
    let auth = serde_json::from_str(&auth_text)
        .with_context(|| format!("invalid JSON in {}", paths.auth.display()))?;
    Ok((config, auth))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_jsonc_and_aliases() {
        let parsed: OpenCodeConfig = json5::from_str(
            r#"{
            // OpenCode permits comments and trailing commas.
            disabled_providers: ["off"],
            provider: { demo: {
                npm: "@ai-sdk/openai-compatible",
                options: { baseURL: "https://example.test/v1", apiKey: "inline", },
                models: { fast: { name: "Fast", reasoning: true } }
            }}
        }"#,
        )
        .unwrap();
        assert_eq!(
            parsed.provider["demo"].options.base_url.as_deref(),
            Some("https://example.test/v1")
        );
        assert_eq!(
            parsed.provider["demo"].options.api_key.as_deref(),
            Some("inline")
        );
        assert!(parsed.provider["demo"].models["fast"].reasoning);
    }
}
