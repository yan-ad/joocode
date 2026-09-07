use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

static JOURNAL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Record {
    #[serde(default)]
    integration: String,
    path: String,
    fingerprint: String,
}

fn record_key(id: &str, path: &Path) -> String {
    format!(
        "{id}:{:x}",
        Sha256::digest(path.to_string_lossy().as_bytes())
    )
}

#[derive(Default, Deserialize, Serialize)]
struct Journal {
    #[serde(default)]
    integrations: BTreeMap<String, Record>,
}

pub fn assert_unchanged(id: &str, target_path: &Path, managed: &Value) -> anyhow::Result<()> {
    assert_unchanged_at(&path()?, id, target_path, managed)
}

pub(crate) fn assert_unchanged_at(
    journal_path: &Path,
    id: &str,
    path: &Path,
    managed: &Value,
) -> anyhow::Result<()> {
    let _guard = JOURNAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("integration journal lock is poisoned"))?;
    let journal = load_at(journal_path)?;
    let target = path.to_string_lossy();
    let Some(record) = journal.integrations.values().find(|record| {
        (record.integration.is_empty() || record.integration == id) && record.path == target
    }) else {
        return Ok(());
    };
    let current = fingerprint(managed)?;
    if current != record.fingerprint {
        anyhow::bail!(
            "managed {id} settings were changed outside Joocode; refusing to overwrite them"
        );
    }
    Ok(())
}

pub fn record(id: &str, target_path: &Path, managed: &Value) -> anyhow::Result<()> {
    record_at(&path()?, id, target_path, managed)
}

pub(crate) fn record_at(
    journal_path: &Path,
    id: &str,
    path: &Path,
    managed: &Value,
) -> anyhow::Result<()> {
    let _guard = JOURNAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("integration journal lock is poisoned"))?;
    let mut journal = load_at(journal_path)?;
    journal.integrations.remove(id);
    journal.integrations.insert(
        record_key(id, path),
        Record {
            integration: id.to_owned(),
            path: path.to_string_lossy().into_owned(),
            fingerprint: fingerprint(managed)?,
        },
    );
    save_at(journal_path, &journal)
}

pub fn remove(id: &str) -> anyhow::Result<()> {
    remove_at(&path()?, id)
}

pub(crate) fn remove_at(journal_path: &Path, id: &str) -> anyhow::Result<()> {
    let _guard = JOURNAL_LOCK
        .lock()
        .map_err(|_| anyhow::anyhow!("integration journal lock is poisoned"))?;
    let mut journal = load_at(journal_path)?;
    let original_len = journal.integrations.len();
    journal
        .integrations
        .retain(|key, record| key != id && record.integration != id);
    if journal.integrations.len() != original_len {
        save_at(journal_path, &journal)?;
    }
    Ok(())
}

fn fingerprint(value: &Value) -> anyhow::Result<String> {
    let bytes = serde_json::to_vec(value)?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("JOOCODE_INTEGRATION_JOURNAL") {
        return Ok(PathBuf::from(path));
    }
    let root = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
        .context("cannot determine Joocode state directory")?;
    Ok(root.join("joocode/integrations.json"))
}

fn load_at(path: &Path) -> anyhow::Result<Journal> {
    match fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .with_context(|| format!("invalid integration journal {}", path.display())),
        Ok(_) => Ok(Journal::default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Journal::default()),
        Err(error) => Err(error).with_context(|| format!("failed reading {}", path.display())),
    }
}

fn save_at(path: &Path, journal: &Journal) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let temporary = path.with_extension(format!("json.{}.tmp", uuid::Uuid::new_v4().simple()));
    fs::write(&temporary, serde_json::to_vec_pretty(journal)?)
        .with_context(|| format!("failed writing {}", temporary.display()))?;
    set_private_permissions(&temporary)?;
    fs::rename(&temporary, path).with_context(|| format!("failed replacing {}", path.display()))?;
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_external_changes_to_managed_values() {
        let directory = tempfile::tempdir().unwrap();
        let journal = directory.path().join("integrations.json");
        let path = directory.path().join("settings.json");
        let managed = serde_json::json!({"provider":{"api_url":"http://localhost"}});
        record_at(&journal, "zed", &path, &managed).unwrap();
        assert_unchanged_at(&journal, "zed", &path, &managed).unwrap();
        assert!(
            assert_unchanged_at(
                &journal,
                "zed",
                &path,
                &serde_json::json!({"provider":{"api_url":"http://changed"}})
            )
            .is_err()
        );
        remove_at(&journal, "zed").unwrap();
    }
}
