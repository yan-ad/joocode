use std::{collections::BTreeSet, fs, path::PathBuf};

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Strategy {
    #[default]
    Failover,
    WeightedRoundRobin,
    LowestLatency,
}

impl From<&str> for ComboModel {
    fn from(model: &str) -> Self {
        Self::Model(model.to_owned())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ComboModel {
    Model(String),
    Weighted { model: String, weight: u32 },
}

impl ComboModel {
    pub fn model(&self) -> &str {
        match self {
            Self::Model(model) | Self::Weighted { model, .. } => model,
        }
    }

    pub fn weight(&self) -> u32 {
        match self {
            Self::Model(_) => 1,
            Self::Weighted { weight, .. } => *weight,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Combo {
    pub name: String,
    #[serde(default)]
    pub strategy: Strategy,
    pub models: Vec<ComboModel>,
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
        let mut models = BTreeSet::new();
        for model in &combo.models {
            if model.model().trim().is_empty() {
                bail!("combo '{name}' contains an empty model ID");
            }
            if !models.insert(model.model()) {
                bail!(
                    "combo '{name}' contains duplicate model '{}'",
                    model.model()
                );
            }
            if model.weight() == 0 {
                bail!(
                    "combo '{name}' model '{}' must have a positive weight",
                    model.model()
                );
            }
        }
    }
    Ok(combos)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_legacy_and_weighted_forms() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("combos.json");
        fs::write(
            &path,
            r#"{"combos":[{"name":"coding","models":["a/model","b/model"]}]}"#,
        )
        .unwrap();
        let combos = load_from(&path).unwrap();
        assert_eq!(combos[0].strategy, Strategy::Failover);
        assert_eq!(combos[0].models[0].model(), "a/model");

        fs::write(
            &path,
            r#"[{"name":"balanced","strategy":"weighted-round-robin","models":[{"model":"a/model","weight":3},{"model":"b/model","weight":1}]}]"#,
        )
        .unwrap();
        let combo = load_from(&path).unwrap().remove(0);
        assert_eq!(combo.strategy, Strategy::WeightedRoundRobin);
        assert_eq!(combo.models[0].weight(), 3);
    }

    #[test]
    fn rejects_invalid_combos() {
        assert!(
            validate(vec![Combo {
                name: "bad/name".into(),
                strategy: Strategy::Failover,
                models: vec![ComboModel::Model("a/model".into())]
            }])
            .is_err()
        );
        assert!(
            validate(vec![Combo {
                name: "empty".into(),
                strategy: Strategy::Failover,
                models: vec![]
            }])
            .is_err()
        );
        assert!(
            validate(vec![Combo {
                name: "zero".into(),
                strategy: Strategy::WeightedRoundRobin,
                models: vec![ComboModel::Weighted {
                    model: "a/model".into(),
                    weight: 0
                }]
            }])
            .is_err()
        );
    }
}
