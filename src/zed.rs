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

fn install_local_api_key(base_url: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        // Zed stores compatible-provider keys as Internet Password records using
        // the configured API URL as the server field. A missing item is normal:
        // the helper creates it in the user's available login/default keychain.
        crate::macos_keychain::ensure_internet_password(PROVIDER_ID, base_url, LOCAL_API_KEY)?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = base_url;
    }

    Ok(())
}

const PROVIDER_ID: &str = "joocode";
const COMMIT_INSTRUCTIONS_START: &str = "<!-- joocode:conventional-commits:start -->";
const COMMIT_INSTRUCTIONS_END: &str = "<!-- joocode:conventional-commits:end -->";
const CONVENTIONAL_COMMITS_INSTRUCTIONS: &str = r#"<!-- joocode:conventional-commits:start -->
Follow Conventional Commits 1.0.0 when generating commit messages:
https://www.conventionalcommits.org/en/v1.0.0/

Use this structure:
<type>[optional scope][optional !]: <description>

- Use `feat` for a new feature and `fix` for a bug fix.
- Other suitable types include `build`, `chore`, `ci`, `docs`, `perf`, `refactor`, `revert`, `style`, and `test`.
- Use an optional lowercase scope when it adds useful context.
- Keep the description concise, imperative, and without a trailing period.
- Add a body only when it provides useful context not present in the subject.
- Mark breaking changes with `!` before `:` or a `BREAKING CHANGE:` footer.
- Return only the commit message, with no markdown fence or commentary.
<!-- joocode:conventional-commits:end -->"#;
#[cfg(target_os = "macos")]
const LOCAL_API_KEY: &str = "joocode-local";

fn merge_commit_instructions(existing: &str) -> String {
    let existing = without_commit_instructions(existing);
    if existing.is_empty() {
        CONVENTIONAL_COMMITS_INSTRUCTIONS.to_owned()
    } else {
        format!("{existing}\n\n{CONVENTIONAL_COMMITS_INSTRUCTIONS}")
    }
}

fn without_commit_instructions(value: &str) -> String {
    let Some(start) = value.find(COMMIT_INSTRUCTIONS_START) else {
        return value.trim().to_owned();
    };
    let Some(relative_end) = value[start..].find(COMMIT_INSTRUCTIONS_END) else {
        return value.trim().to_owned();
    };
    let end = start + relative_end + COMMIT_INSTRUCTIONS_END.len();
    format!("{}{}", &value[..start], &value[end..])
        .trim()
        .to_owned()
}

fn remove_commit_instructions(root: &mut Value) {
    let Some(agent) = root.get_mut("agent").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(existing) = agent
        .get("commit_message_instructions")
        .and_then(Value::as_str)
    else {
        return;
    };
    let remaining = without_commit_instructions(existing);
    if remaining.is_empty() {
        agent.remove("commit_message_instructions");
    } else {
        agent.insert(
            "commit_message_instructions".into(),
            Value::String(remaining),
        );
    }
}

/// Add or replace Joocode's own provider entry without changing unrelated Zed
/// preferences. On macOS, this also adds a non-secret local placeholder key to
/// Zed's keychain. Zed only shows compatible-provider models after it finds an
/// API key; Joocode ignores this value and continues using source credentials.
pub fn install(registry: &Registry, base_url: &str) -> anyhow::Result<PathBuf> {
    let path = settings_path()?;
    let default_model = crate::local_config::default_model_route()?;
    let path = install_at(registry, base_url, path, default_model.as_deref())?;
    install_local_api_key(base_url)?;
    Ok(path)
}

pub fn uninstall() -> anyhow::Result<()> {
    let path = settings_path()?;
    if !path.is_file() {
        return Ok(());
    }
    let text = fs::read_to_string(&path)?;
    let mut root: Value = json5::from_str(&text)
        .with_context(|| format!("invalid Zed settings JSONC at {}", path.display()))?;
    if let Some(compatible) = root
        .pointer_mut("/language_models/openai_compatible")
        .and_then(Value::as_object_mut)
    {
        compatible.remove(PROVIDER_ID);
        compatible.remove("joc");
        compatible.remove("crabcodex");
    }
    remove_commit_instructions(&mut root);
    fs::write(&path, format!("{}\n", serde_json::to_string_pretty(&root)?))?;
    Ok(())
}

fn install_at(
    registry: &Registry,
    base_url: &str,
    path: PathBuf,
    default_model: Option<&str>,
) -> anyhow::Result<PathBuf> {
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
    let agent = object_at(root, "agent")?;
    let existing_instructions = agent
        .get("commit_message_instructions")
        .and_then(Value::as_str)
        .unwrap_or_default();
    agent.insert(
        "commit_message_instructions".into(),
        Value::String(merge_commit_instructions(existing_instructions)),
    );
    if let Some(model) = default_model {
        let selection = json!({ "provider": PROVIDER_ID, "model": model });
        agent.insert("default_model".into(), selection.clone());
        agent.insert("commit_message_model".into(), selection);
    }
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

        install_at(
            &registry,
            "http://127.0.0.1:10100/v1",
            settings.clone(),
            None,
        )
        .unwrap();

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
    fn installs_default_and_commit_message_model() {
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
        let registry = Registry::load(&ConfigPaths { config, auth }).unwrap();

        install_at(
            &registry,
            "http://127.0.0.1:10100/v1",
            settings.clone(),
            Some("joocode/demo/fast"),
        )
        .unwrap();

        let result: Value = serde_json::from_str(&fs::read_to_string(settings).unwrap()).unwrap();
        let expected = json!({"provider":"joocode","model":"joocode/demo/fast"});
        assert_eq!(result["agent"]["default_model"], expected);
        assert_eq!(result["agent"]["commit_message_model"], expected);
        let instructions = result["agent"]["commit_message_instructions"]
            .as_str()
            .unwrap();
        assert!(instructions.contains("Conventional Commits 1.0.0"));
        assert!(instructions.contains("https://www.conventionalcommits.org/en/v1.0.0/"));
        assert!(instructions.contains("<type>[optional scope][optional !]: <description>"));
    }

    #[test]
    fn commit_instructions_are_idempotent_and_preserve_user_context() {
        let custom = "Use the repository's preferred package scope.";
        let first = merge_commit_instructions(custom);
        let second = merge_commit_instructions(&first);

        assert!(second.starts_with(custom));
        assert_eq!(second.matches(COMMIT_INSTRUCTIONS_START).count(), 1);
        assert_eq!(second.matches(COMMIT_INSTRUCTIONS_END).count(), 1);
        assert_eq!(without_commit_instructions(&second), custom);
    }

    #[test]
    fn removing_joocode_commit_context_keeps_user_instructions() {
        let custom = "Mention the issue number when one is available.";
        let mut root = json!({
            "agent": {
                "commit_message_instructions": merge_commit_instructions(custom)
            }
        });

        remove_commit_instructions(&mut root);

        assert_eq!(root["agent"]["commit_message_instructions"], custom);
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn local_api_key_is_a_non_secret_placeholder() {
        assert_eq!(PROVIDER_ID, "joocode");
        assert_eq!(LOCAL_API_KEY, "joocode-local");
    }
}
