use std::{env, fs, path::PathBuf};

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

/// Add or replace JustOpenCode's own provider entry without changing unrelated Zed
/// preferences or any provider credentials stored in Zed's keychain.
pub fn install(registry: &Registry, base_url: &str) -> anyhow::Result<PathBuf> {
    let path = settings_path()?;
    install_at(registry, base_url, path)
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
                    "chat_completions": true
                }
            });
            if model.reasoning {
                entry["reasoning_effort"] = Value::String("medium".into());
            }
            entry
        })
        .collect::<Vec<_>>();
    compatible.insert(
        "joc".into(),
        json!({ "api_url": base_url, "available_models": models }),
    );
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
            result["language_models"]["openai_compatible"]["joc"]["api_url"],
            "http://127.0.0.1:10100/v1"
        );
        assert_eq!(
            result["language_models"]["openai_compatible"]["joc"]["available_models"][0]["name"],
            "demo/fast"
        );
    }
}
