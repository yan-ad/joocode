use std::{fs, path::PathBuf};

use anyhow::Context;
use serde_json::{Map, Value, json};

use crate::provider::Registry;

const LOCAL_TOKEN: &str = "joocode-local";

pub fn install(registry: &Registry, base_url: &str) -> anyhow::Result<PathBuf> {
    let path = settings_path()?;
    let mut root = read_settings(&path)?;
    let object = root
        .as_object_mut()
        .context("Claude Code settings root must be an object")?;
    let env = object_at(object, "env")?;
    env.insert(
        "ANTHROPIC_BASE_URL".into(),
        Value::String(
            base_url
                .trim_end_matches('/')
                .trim_end_matches("/v1")
                .to_owned(),
        ),
    );
    env.insert(
        "ANTHROPIC_AUTH_TOKEN".into(),
        Value::String(LOCAL_TOKEN.into()),
    );
    env.insert(
        "CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY".into(),
        Value::String("1".into()),
    );
    if let Some(model) = registry.models().first() {
        let model = format!("claude-joocode/{}", model.id);
        env.insert("ANTHROPIC_MODEL".into(), Value::String(model.clone()));
        env.insert("ANTHROPIC_SMALL_FAST_MODEL".into(), Value::String(model));
    }
    write_settings(&path, &root)?;
    Ok(path)
}

pub fn uninstall() -> anyhow::Result<()> {
    let path = settings_path()?;
    if !path.is_file() {
        return Ok(());
    }
    let mut root = read_settings(&path)?;
    if let Some(env) = root.get_mut("env").and_then(Value::as_object_mut)
        && env.get("ANTHROPIC_AUTH_TOKEN").and_then(Value::as_str) == Some(LOCAL_TOKEN)
    {
        env.remove("ANTHROPIC_AUTH_TOKEN");
        env.remove("ANTHROPIC_BASE_URL");
        env.remove("CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY");
        for key in ["ANTHROPIC_MODEL", "ANTHROPIC_SMALL_FAST_MODEL"] {
            if env
                .get(key)
                .and_then(Value::as_str)
                .is_some_and(|model| model.starts_with("claude-joocode/"))
            {
                env.remove(key);
            }
        }
    }
    write_settings(&path, &root)
}

fn settings_path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("CLAUDE_SETTINGS_PATH").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let root = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
        .context("could not resolve Claude Code config directory")?;
    Ok(root.join("settings.json"))
}

fn read_settings(path: &std::path::Path) -> anyhow::Result<Value> {
    match fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => json5::from_str(&text)
            .with_context(|| format!("invalid Claude Code settings JSON at {}", path.display())),
        Ok(_) => Ok(json!({})),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error).with_context(|| format!("failed reading {}", path.display())),
    }
}

fn write_settings(path: &std::path::Path, root: &Value) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("Claude Code settings path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed creating {}", parent.display()))?;
    fs::write(path, format!("{}\n", serde_json::to_string_pretty(root)?))
        .with_context(|| format!("failed writing {}", path.display()))
}

fn object_at<'a>(
    parent: &'a mut Map<String, Value>,
    key: &str,
) -> anyhow::Result<&'a mut Map<String, Value>> {
    parent
        .entry(key.to_owned())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .with_context(|| format!("Claude Code setting '{key}' must be an object"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_removes_only_joocode_gateway_values() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, r#"{"theme":"dark","env":{"KEEP":"yes"}}"#).unwrap();
        let mut root = read_settings(&path).unwrap();
        let env = object_at(root.as_object_mut().unwrap(), "env").unwrap();
        env.insert("ANTHROPIC_BASE_URL".into(), json!("http://127.0.0.1:10100"));
        env.insert("ANTHROPIC_AUTH_TOKEN".into(), json!(LOCAL_TOKEN));
        write_settings(&path, &root).unwrap();
        let root = read_settings(&path).unwrap();
        assert_eq!(root["theme"], "dark");
        assert_eq!(root["env"]["KEEP"], "yes");
        assert_eq!(root["env"]["ANTHROPIC_AUTH_TOKEN"], LOCAL_TOKEN);
    }
}
