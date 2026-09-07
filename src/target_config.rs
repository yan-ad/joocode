use std::{
    collections::{BTreeMap, BTreeSet},
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
    #[serde(default)]
    pub disabled_local_providers: BTreeSet<String>,
    #[serde(default)]
    pub disabled_models: BTreeSet<String>,
    #[serde(default)]
    pub subagent_catalog: SubagentCatalogPolicy,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SubagentCatalogPolicy {
    #[serde(default)]
    pub featured_models: Vec<String>,
    #[serde(default)]
    pub fallback_models: Vec<String>,
    #[serde(default = "default_subagent_max_entries")]
    pub max_entries: usize,
    #[serde(default)]
    pub reasoning_effort_cap: Option<ReasoningEffortCap>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffortCap {
    Low,
    Medium,
    High,
}

impl ReasoningEffortCap {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

const fn default_subagent_max_entries() -> usize {
    12
}

impl Default for SubagentCatalogPolicy {
    fn default() -> Self {
        Self {
            featured_models: Vec::new(),
            fallback_models: Vec::new(),
            max_entries: default_subagent_max_entries(),
            reasoning_effort_cap: None,
        }
    }
}

impl SubagentCatalogPolicy {
    pub fn cycle_reasoning_effort_cap(&mut self) {
        self.reasoning_effort_cap = match self.reasoning_effort_cap {
            None => Some(ReasoningEffortCap::Low),
            Some(ReasoningEffortCap::Low) => Some(ReasoningEffortCap::Medium),
            Some(ReasoningEffortCap::Medium) => Some(ReasoningEffortCap::High),
            Some(ReasoningEffortCap::High) => None,
        };
    }

    pub fn resolve(
        &self,
        models: &[crate::provider::ModelInfo],
    ) -> Vec<crate::provider::ModelInfo> {
        let by_id = models
            .iter()
            .map(|model| (model.id.as_str(), model))
            .collect::<BTreeMap<_, _>>();
        let mut seen = BTreeSet::new();
        self.featured_models
            .iter()
            .chain(&self.fallback_models)
            .filter_map(|id| by_id.get(id.as_str()).copied())
            .filter(|model| seen.insert(model.id.as_str()))
            .take(self.max_entries)
            .cloned()
            .collect()
    }
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
            disabled_local_providers: BTreeSet::new(),
            disabled_models: BTreeSet::new(),
            subagent_catalog: SubagentCatalogPolicy::default(),
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

    pub fn local_provider_enabled(&self, provider: &str) -> bool {
        !self.disabled_local_providers.contains(provider)
    }

    pub fn set_local_provider(provider: &str, enabled: bool) -> anyhow::Result<Self> {
        let path = path()?;
        let mut preferences = load_from(&path)?;
        if enabled {
            preferences.disabled_local_providers.remove(provider);
        } else {
            preferences
                .disabled_local_providers
                .insert(provider.to_owned());
        }
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

    pub fn set_disabled_models(disabled_models: BTreeSet<String>) -> anyhow::Result<Self> {
        let path = path()?;
        let mut preferences = load_from(&path)?;
        preferences.disabled_models = disabled_models;
        save_to(&path, &preferences)?;
        Ok(preferences)
    }

    pub fn set_subagent_catalog(policy: SubagentCatalogPolicy) -> anyhow::Result<Self> {
        let path = path()?;
        let mut preferences = load_from(&path)?;
        preferences.subagent_catalog = policy;
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
    fn local_provider_toggle_defaults_on_and_persists_off() {
        let preferences = TargetPreferences::default();
        assert!(preferences.local_provider_enabled("openai"));
        let mut preferences = preferences;
        preferences.disabled_local_providers.insert("openai".into());
        assert!(!preferences.local_provider_enabled("openai"));
    }

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
        preferences.disabled_models.insert("demo/hidden".into());
        preferences.subagent_catalog = SubagentCatalogPolicy {
            featured_models: vec!["demo/featured".into()],
            fallback_models: vec!["demo/fallback".into()],
            max_entries: 2,
            reasoning_effort_cap: None,
        };
        save_to(&path, &preferences).unwrap();

        let loaded = load_from(&path).unwrap();
        assert!(!loaded.run_in_background);
        assert_eq!(loaded.override_for(ProxyTarget::Codex), Some(false));
        assert_eq!(loaded.override_for(ProxyTarget::GrokBuild), Some(true));
        assert_eq!(loaded.override_for(ProxyTarget::Zed), None);
        assert_eq!(loaded.source_override(SourceKind::OpenCode), Some(false));
        assert_eq!(loaded.source_override(SourceKind::CrabCode), None);
        assert!(loaded.disabled_models.contains("demo/hidden"));
        assert_eq!(loaded.subagent_catalog.featured_models, ["demo/featured"]);
        assert_eq!(loaded.subagent_catalog.fallback_models, ["demo/fallback"]);
        assert_eq!(loaded.subagent_catalog.max_entries, 2);
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

    #[test]
    fn subagent_catalog_keeps_configured_order_and_valid_models() {
        let model = |id: &str| crate::provider::ModelInfo {
            id: id.into(),
            provider: "fixture".into(),
            upstream_id: id.into(),
            name: id.into(),
            reasoning: false,
            context_window: None,
            max_output_tokens: None,
        };
        let policy = SubagentCatalogPolicy {
            featured_models: vec!["b".into(), "missing".into(), "a".into()],
            fallback_models: vec!["a".into(), "c".into()],
            max_entries: 3,
            reasoning_effort_cap: None,
        };
        let ids = policy
            .resolve(&[model("a"), model("b"), model("c")])
            .into_iter()
            .map(|model| model.id)
            .collect::<Vec<_>>();
        assert_eq!(ids, vec!["b", "a", "c"]);
    }
}
