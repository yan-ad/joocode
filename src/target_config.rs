use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::sources::SourceKind;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxyTarget {
    Codex,
    GitHubCopilotApp,
    JetBrains,
    Antigravity,
    Zed,
    ClaudeCode,
    GrokBuild,
}

impl ProxyTarget {
    pub const ALL: [Self; 7] = [
        Self::Codex,
        Self::GitHubCopilotApp,
        Self::JetBrains,
        Self::Antigravity,
        Self::Zed,
        Self::ClaudeCode,
        Self::GrokBuild,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::GitHubCopilotApp => "GitHub Copilot App",
            Self::JetBrains => "JetBrains",
            Self::Antigravity => "Antigravity",
            Self::Zed => "Zed",
            Self::ClaudeCode => "Claude Code",
            Self::GrokBuild => "Grok Build",
        }
    }

    pub const fn support_note(self) -> Option<&'static str> {
        match self {
            Self::JetBrains => Some("manual credential"),
            Self::Antigravity => Some("patched app · macOS"),
            Self::ClaudeCode => Some("experimental"),
            Self::Codex | Self::GitHubCopilotApp | Self::Zed | Self::GrokBuild => None,
        }
    }

    pub const fn can_auto_configure(self) -> bool {
        !matches!(self, Self::Antigravity) || cfg!(target_os = "macos")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TargetPreferences {
    #[serde(default = "default_run_in_background")]
    pub run_in_background: bool,
    #[serde(default)]
    pub proxy_to: BTreeMap<ProxyTarget, bool>,
    #[serde(default)]
    pub detected_providers: BTreeMap<String, bool>,
}

const fn default_run_in_background() -> bool {
    true
}

impl Default for TargetPreferences {
    fn default() -> Self {
        Self {
            run_in_background: true,
            proxy_to: BTreeMap::new(),
            detected_providers: BTreeMap::new(),
        }
    }
}

impl TargetPreferences {
    pub fn load() -> anyhow::Result<Self> {
        load_from(&path()?)
    }

    pub fn override_for(&self, target: ProxyTarget) -> Option<bool> {
        self.proxy_to.get(&target).copied()
    }

    pub fn set(target: ProxyTarget, enabled: bool) -> anyhow::Result<Self> {
        let path = path()?;
        let mut preferences = load_from(&path)?;
        preferences.proxy_to.insert(target, enabled);
        save_to(&path, &preferences)?;
        Ok(preferences)
    }

    pub fn set_run_in_background(enabled: bool) -> anyhow::Result<Self> {
        let path = path()?;
        let mut preferences = load_from(&path)?;
        preferences.run_in_background = enabled;
        save_to(&path, &preferences)?;
        Ok(preferences)
    }

    pub fn source_override(&self, source: SourceKind) -> Option<bool> {
        self.detected_providers.get(source.key()).copied()
    }

    pub fn set_source(source: SourceKind, enabled: bool) -> anyhow::Result<Self> {
        let path = path()?;
        let mut preferences = load_from(&path)?;
        preferences
            .detected_providers
            .insert(source.key().to_owned(), enabled);
        save_to(&path, &preferences)?;
        Ok(preferences)
    }
}

pub fn path() -> anyhow::Result<PathBuf> {
    if let Some(path) = std::env::var_os("JOOCODE_SETTINGS").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .context("cannot determine Joocode settings directory")?;
    Ok(root.join("joocode/settings.json"))
}

fn load_from(path: &Path) -> anyhow::Result<TargetPreferences> {
    match fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str(&text)
            .with_context(|| format!("invalid JSON in {}", path.display())),
        Ok(_) => Ok(TargetPreferences::default()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(TargetPreferences::default())
        }
        Err(error) => Err(error).with_context(|| format!("failed reading {}", path.display())),
    }
}

fn save_to(path: &Path, preferences: &TargetPreferences) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed creating {}", parent.display()))?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec_pretty(preferences)?)
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
    fn preferences_round_trip() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        let mut preferences = TargetPreferences {
            run_in_background: false,
            ..TargetPreferences::default()
        };
        preferences.proxy_to.insert(ProxyTarget::Codex, false);
        preferences.proxy_to.insert(ProxyTarget::GrokBuild, true);
        preferences
            .detected_providers
            .insert(SourceKind::OpenCode.key().into(), false);
        save_to(&path, &preferences).unwrap();

        let loaded = load_from(&path).unwrap();
        assert!(!loaded.run_in_background);
        assert_eq!(loaded.override_for(ProxyTarget::Codex), Some(false));
        assert_eq!(loaded.override_for(ProxyTarget::GrokBuild), Some(true));
        assert_eq!(loaded.override_for(ProxyTarget::Zed), None);
        assert_eq!(loaded.source_override(SourceKind::OpenCode), Some(false));
        assert_eq!(loaded.source_override(SourceKind::CrabCode), None);
    }

    #[test]
    fn missing_background_preference_defaults_on() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "{}").unwrap();

        assert!(load_from(&path).unwrap().run_in_background);
    }

    #[test]
    fn target_order_matches_configuration_menu() {
        assert_eq!(
            ProxyTarget::ALL.map(ProxyTarget::label),
            [
                "Codex",
                "GitHub Copilot App",
                "JetBrains",
                "Antigravity",
                "Zed",
                "Claude Code",
                "Grok Build",
            ]
        );
    }
}
