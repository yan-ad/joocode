use std::{collections::BTreeSet, fs, path::PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Combo {
    pub name: String,
    pub models: Vec<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ComboFile {
    List(Vec<Combo>),
    Object { combos: Vec<Combo> },
}

pub fn path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("JOOCODE_COMBOS") {
        return Ok(PathBuf::from(path));
    }
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .context("cannot determine Joocode config directory")?;
    Ok(root.join("joocode/combos.json"))
}

pub fn load() -> anyhow::Result<Vec<Combo>> {
    load_from(&path()?)
}

fn load_from(path: &std::path::Path) -> anyhow::Result<Vec<Combo>> {
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed reading combo configuration {}", path.display()))?;
    let file: ComboFile = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON in combo configuration {}", path.display()))?;
    let combos = match file {
        ComboFile::List(combos) | ComboFile::Object { combos } => combos,
    };
    validate(combos)
}

fn validate(combos: Vec<Combo>) -> anyhow::Result<Vec<Combo>> {
    let mut names = BTreeSet::new();
    for combo in &combos {
        let name = combo.name.trim();
        if name.is_empty() || name.contains('/') {
            bail!("combo name must be non-empty and cannot contain '/'");
        }
        if !names.insert(name.to_owned()) {
            bail!("duplicate combo name '{name}'");
        }
        if combo.models.is_empty() {
            bail!("combo '{name}' must contain at least one model");
        }
    }
    Ok(combos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_list_and_object_forms() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("combos.json");
        fs::write(
            &path,
            r#"{"combos":[{"name":"coding","models":["a/model","b/model"]}]}"#,
        )
        .unwrap();
        let combos = load_from(&path).unwrap();
        assert_eq!(combos[0].name, "coding");
        assert_eq!(combos[0].models.len(), 2);

        fs::write(&path, r#"[{"name":"fast","models":["a/model"]}]"#).unwrap();
        assert_eq!(load_from(&path).unwrap()[0].name, "fast");
    }

    #[test]
    fn rejects_invalid_combos() {
        assert!(
            validate(vec![Combo {
                name: "bad/name".into(),
                models: vec!["a/model".into()]
            }])
            .is_err()
        );
        assert!(
            validate(vec![Combo {
                name: "empty".into(),
                models: vec![]
            }])
            .is_err()
        );
    }
}
