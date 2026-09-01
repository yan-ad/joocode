use std::{env, fs, path::PathBuf};

#[cfg(target_os = "macos")]
use std::process::Command;

use anyhow::Context;
use serde_json::{Map, Value, json};

use crate::provider::Registry;

fn settings_path() -> anyhow::Result<PathBuf> {
    if let Some(path) = env::var_os("ZED_SETTINGS_PATH").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    Ok(dirs::home_dir()
        .context("could not resolve the user home directory")?
        .join(".config/zed/settings.json"))
}

fn install_local_api_key(base_url: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let security = if PathBuf::from("/usr/bin/security").is_file() {
            "/usr/bin/security"
        } else {
            "security"
        };
        let existing = Command::new(security)
            .args(["find-internet-password", "-a", PROVIDER_ID, "-s", base_url])
            .output()
            .context("failed to inspect Joocode's local Zed API key")?;
        if existing.status.success() {
            return Ok(());
        }

        // Zed stores compatible-provider keys as Internet Password records using
        // the configured API URL as the server field. `security` is the macOS
        // supported command-line interface to that keychain record type.
        let output = Command::new(security)
            .args([
                "add-internet-password",
                "-a",
                PROVIDER_ID,
                "-s",
                base_url,
                "-w",
                LOCAL_API_KEY,
                "-U",
            ])
            .output()
            .context("failed to store Joocode's local Zed API key in the macOS keychain")?;
        anyhow::ensure!(
            output.status.success(),
            "failed to store Joocode's local Zed API key: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = base_url;
    }

    Ok(())
}

const PROVIDER_ID: &str = "joocode";
#[cfg(target_os = "macos")]
const LOCAL_API_KEY: &str = "joocode-local";

/// Add or replace Joocode's own provider entry without changing unrelated Zed
/// preferences. On macOS, this also adds a non-secret local placeholder key to
/// Zed's keychain. Zed only shows compatible-provider models after it finds an
/// API key; Joocode ignores this value and continues using source credentials.
pub fn install(registry: &Registry, base_url: &str) -> anyhow::Result<PathBuf> {
    let path = settings_path()?;
    let path = install_at(registry, base_url, path)?;
    install_local_api_key(base_url)?;
    Ok(path)
}

fn install_at(registry: &Registry, base_url: &str, path: PathBuf) -> anyhow::Result<PathBuf> {
    let mut root = match fs::read_to_string(&path) {
        Ok(text) if !text.trim().is_empty() => json5::from_str::<Value>(&text)
            .with_context(|| format!("invalid Zed settings JSONC at {}", path.display()))?,
        Ok(_) => json!({}),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(error) => {
            return Err(error).with_context(|| format!("failed reading {}", path.display()));
        }
    };
    let root = root
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Zed settings root must be an object"))?;
    let language_models = object_at(root, "language_models")?;
    let compatible = object_at(language_models, "openai_compatible")?;
    let models = registry
        .models()
        .iter()
        .map(|model| {
            let mut entry = json!({
                "name": model.id,
                "display_name": model.id,
                "max_tokens": model.context_window.unwrap_or(128_000),
                "max_completion_tokens": model.max_output_tokens.unwrap_or(16_384),
                "capabilities": {
                    "tools": true,
                    "images": true,
                    "parallel_tool_calls": true,
                    "chat_completions": true,
                    "interleaved_reasoning": false,
                    "max_tokens_parameter": true,
                    "prompt_cache_key": false
                }
            });
            if model.reasoning {
                entry["reasoning_effort"] = Value::String("medium".into());
            }
            entry
        })
        .collect::<Vec<_>>();
    compatible.insert(
        PROVIDER_ID.into(),
        json!({ "api_url": base_url, "available_models": models }),
    );
    compatible.remove("joc");
    compatible.remove("crabcodex");
    let parent = path.parent().context("Zed settings path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed creating {}", parent.display()))?;
    let content = format!("{}\n", serde_json::to_string_pretty(&root)?);
    fs::write(&path, content).with_context(|| format!("failed writing {}", path.display()))?;
    Ok(path)
}

fn object_at<'a>(
    parent: &'a mut Map<String, Value>,
    key: &str,
) -> anyhow::Result<&'a mut Map<String, Value>> {
    let value = parent.entry(key.to_owned()).or_insert_with(|| json!({}));
    value
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Zed setting '{key}' must be an object"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigPaths;
    use crate::provider::Registry;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn object_at_rejects_non_objects() {
        let mut root = Map::from_iter([("language_models".into(), json!(false))]);
        assert!(object_at(&mut root, "language_models").is_err());
    }

    #[test]
    fn installs_qualified_models_without_replacing_other_settings() {
        let dir = tempdir().unwrap();
        let config = dir.path().join("opencode.jsonc");
        let auth = dir.path().join("auth.json");
        let settings = dir.path().join("zed/settings.json");
        fs::write(
            &config,
            r#"{ provider: { demo: { options: { baseURL: "https://upstream.test/v1" }, models: { fast: {} } } } }"#,
        )
        .unwrap();
        fs::write(&auth, "{}").unwrap();
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(
            &settings,
            r#"{"theme":"Ayu","language_models":{"openai":{"x":1}}}"#,
        )
        .unwrap();
        let registry = Registry::load(&ConfigPaths { config, auth }).unwrap();

        install_at(&registry, "http://127.0.0.1:10100/v1", settings.clone()).unwrap();

        let result: Value = serde_json::from_str(&fs::read_to_string(settings).unwrap()).unwrap();
        assert_eq!(result["theme"], "Ayu");
        assert_eq!(result["language_models"]["openai"]["x"], 1);
        assert_eq!(
            result["language_models"]["openai_compatible"]["joocode"]["api_url"],
            "http://127.0.0.1:10100/v1"
        );
        assert_eq!(
            result["language_models"]["openai_compatible"]["joocode"]["available_models"][0]["name"],
            "demo/fast"
        );
        assert_eq!(
            result["language_models"]["openai_compatible"]["joocode"]["available_models"][0]["capabilities"]
                ["prompt_cache_key"],
            false
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn local_api_key_is_a_non_secret_placeholder() {
        assert_eq!(PROVIDER_ID, "joocode");
        assert_eq!(LOCAL_API_KEY, "joocode-local");
    }
}
