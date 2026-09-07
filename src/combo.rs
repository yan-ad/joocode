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

fn write_to(path: &std::path::Path, combos: &[Combo]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(combos)?)
        .with_context(|| format!("failed writing {}", temporary.display()))?;
    set_private_permissions(&temporary)?;
    fs::rename(&temporary, path).with_context(|| format!("failed replacing {}", path.display()))?;
    Ok(())
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

pub fn save(combo: Combo, original_name: Option<&str>) -> anyhow::Result<PathBuf> {
    let path = path()?;
    save_to(&path, combo, original_name)?;
    Ok(path)
}

fn save_to(
    path: &std::path::Path,
    combo: Combo,
    original_name: Option<&str>,
) -> anyhow::Result<()> {
    let mut combos = load_from(path)?;
    if let Some(original_name) = original_name {
        combos.retain(|existing| existing.name != original_name);
    }
    if combos.iter().any(|existing| existing.name == combo.name) {
        bail!("combo '{}' already exists", combo.name);
    }
    combos.push(combo);
    combos.sort_by(|left, right| left.name.cmp(&right.name));
    let combos = validate(combos)?;
    write_to(path, &combos)
}

pub fn remove(name: &str) -> anyhow::Result<PathBuf> {
    let path = path()?;
    remove_from(&path, name)?;
    Ok(path)
}

fn remove_from(path: &std::path::Path, name: &str) -> anyhow::Result<()> {
    let mut combos = load_from(path)?;
    let original_len = combos.len();
    combos.retain(|combo| combo.name != name);
    if combos.len() == original_len {
        bail!("combo '{name}' was not found");
    }
    write_to(path, &combos)
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

        fs::write(
            &path,
            r#"[{"name":"fastest","strategy":"lowest-latency","models":["a/model","b/model"]}]"#,
        )
        .unwrap();
        assert_eq!(
            load_from(&path).unwrap()[0].strategy,
            Strategy::LowestLatency
        );
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

    #[test]
    fn saves_updates_and_removes_combos_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("combos.json");
        assert!(remove_from(&path, "coding").is_err());

        let combo = Combo {
            name: "coding".into(),
            strategy: Strategy::Failover,
            models: vec!["a/model".into(), "b/model".into()],
        };
        save_to(&path, combo.clone(), None).unwrap();
        assert_eq!(load_from(&path).unwrap()[0].name, "coding");

        let mut updated = combo;
        updated.strategy = Strategy::LowestLatency;
        save_to(&path, updated, Some("coding")).unwrap();
        assert_eq!(
            load_from(&path).unwrap()[0].strategy,
            Strategy::LowestLatency
        );

        write_to(&path, &[]).unwrap();
        assert!(load_from(&path).unwrap().is_empty());
    }
}
