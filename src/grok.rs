use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use toml_edit::{DocumentMut, Item, Table, value};

use crate::provider::Registry;

const MODEL_PREFIX: &str = "joocode/";

pub fn install(registry: &Registry, base_url: &str) -> anyhow::Result<PathBuf> {
    let path = config_path()?;
    let mut document = read_document(&path)?;
    if document.get("model").is_none() {
        document.insert("model", Item::Table(Table::new()));
    }
    let models = document["model"]
        .as_table_mut()
        .context("Grok Build model config must be a TOML table")?;
    remove_managed_models(models);
    for model in registry.models() {
        let mut entry = Table::new();
        entry["model"] = value(model.id.as_str());
        entry["base_url"] = value(base_url.trim_end_matches('/'));
        entry["name"] = value(model.id.as_str());
        entry["description"] = value(format!("{} via Joocode", model.name));
        entry["api_key"] = value("joocode-local");
        entry["api_backend"] = value("chat_completions");
        if let Some(context_window) = model.context_window {
            entry["context_window"] = value(i64::try_from(context_window).unwrap_or(i64::MAX));
        }
        if let Some(max_tokens) = model.max_output_tokens {
            entry["max_completion_tokens"] = value(i64::try_from(max_tokens).unwrap_or(i64::MAX));
        }
        models[&model.id] = Item::Table(entry);
    }
    write_document(&path, &document)?;
    Ok(path)
}

pub fn uninstall() -> anyhow::Result<()> {
    let path = config_path()?;
    if !path.is_file() {
        return Ok(());
    }
    let mut document = read_document(&path)?;
    if let Some(models) = document.get_mut("model").and_then(Item::as_table_mut) {
        remove_managed_models(models);
    }
    write_document(&path, &document)
}

fn remove_managed_models(models: &mut Table) {
    let managed = models
        .iter()
        .filter_map(|(key, item)| {
            let table = item.as_table()?;
            let managed = key.starts_with(MODEL_PREFIX)
                || table.get("api_key").and_then(Item::as_str) == Some("joocode-local")
                || table
                    .get("description")
                    .and_then(Item::as_str)
                    .is_some_and(|description| description.ends_with(" via Joocode"));
            managed.then(|| key.to_owned())
        })
        .collect::<Vec<_>>();
    for key in managed {
        models.remove(&key);
    }
}

fn config_path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("GROK_CONFIG").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    Ok(dirs::home_dir()
        .context("could not resolve the user home directory")?
        .join(".grok/config.toml"))
}

fn read_document(path: &Path) -> anyhow::Result<DocumentMut> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .parse::<DocumentMut>()
            .with_context(|| format!("invalid TOML in {}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(error) => Err(error).with_context(|| format!("failed reading {}", path.display())),
    }
}

fn write_document(path: &Path, document: &DocumentMut) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("Grok Build config path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed creating {}", parent.display()))?;
    fs::write(path, document.to_string())
        .with_context(|| format!("failed writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::ConfigPaths, provider::Registry};

    #[test]
    fn installs_and_removes_only_joocode_models() {
        let directory = tempfile::tempdir().unwrap();
        let config = directory.path().join("opencode.jsonc");
        let auth = directory.path().join("auth.json");
        let grok = directory.path().join("config.toml");
        fs::write(
            &config,
            r#"{ provider: { demo: { options: { baseURL: "https://upstream.test/v1" }, models: { fast: {} } } } }"#,
        )
        .unwrap();
        fs::write(&auth, "{}").unwrap();
        fs::write(&grok, "[model.native]\nmodel = \"grok-4\"\n").unwrap();
        let registry = Registry::load(&ConfigPaths { config, auth }).unwrap();

        let mut document = read_document(&grok).unwrap();
        let models = document["model"].as_table_mut().unwrap();
        for model in registry.models() {
            let mut entry = Table::new();
            entry["model"] = value(model.id.as_str());
            entry["api_key"] = value("joocode-local");
            models[&model.id] = Item::Table(entry);
        }
        write_document(&grok, &document).unwrap();
        let text = fs::read_to_string(&grok).unwrap();
        assert!(text.contains("demo/fast"));
        assert!(text.contains("model.native"));

        let mut document = read_document(&grok).unwrap();
        remove_managed_models(document["model"].as_table_mut().unwrap());
        write_document(&grok, &document).unwrap();
        let text = fs::read_to_string(&grok).unwrap();
        assert!(!text.contains("demo/fast"));
        assert!(text.contains("model.native"));
    }
}
