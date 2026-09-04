use std::{env, path::PathBuf, process::Command};

use crate::{
    antigravity, claude, codex, copilot_app, grok,
    provider::Registry,
    target_config::{ProxyTarget, TargetPreferences},
    zed,
};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DesktopTargets {
    pub codex: bool,
    pub github_copilot_app: bool,
    pub zed: bool,
    pub jetbrains: bool,
    pub antigravity: bool,
    pub claude_code: bool,
    pub grok_build: bool,
}

fn zed_installed() -> bool {
    if command_exists("zed") || application_exists(&["Zed.app", "Zed Preview.app"]) {
        return true;
    }

    #[cfg(target_os = "windows")]
    {
        return env::var_os("LOCALAPPDATA").is_some_and(|root| {
            let root = PathBuf::from(root);
            root.join("Programs/Zed/Zed.exe").exists() || root.join("Zed/Zed.exe").exists()
        });
    }

    #[cfg(target_os = "linux")]
    {
        return dirs::home_dir().is_some_and(|home| {
            home.join(".local/bin/zed").exists()
                || home.join(".local/share/zed/zed.app/bin/zed").exists()
        }) || PathBuf::from("/usr/bin/zed").exists()
            || PathBuf::from("/usr/local/bin/zed").exists();
    }

    #[allow(unreachable_code)]
    false
}

impl DesktopTargets {
    pub fn detect() -> Self {
        let detected = Self {
            codex: command_exists("codex") || application_exists(&["Codex.app"]),
            github_copilot_app: copilot_app::installed(),
            zed: zed_installed(),
            jetbrains: jetbrains_installed(),
            // Antigravity requires an explicit app clone/patch. Detect an
            // existing patched app, but never create the 400+ MiB clone just
            // because Google's original application is installed.
            antigravity: antigravity::patch_installed(),
            claude_code: command_exists("claude") || application_exists(&["Claude.app"]),
            grok_build: command_exists("grok")
                || application_exists(&["Grok.app", "Grok Build.app"]),
        };
        detected.with_preferences(&TargetPreferences::load().unwrap_or_default())
    }

    pub fn all_supported() -> Self {
        Self {
            codex: true,
            github_copilot_app: true,
            zed: true,
            jetbrains: true,
            antigravity: cfg!(target_os = "macos"),
            claude_code: true,
            grok_build: true,
        }
    }

    pub fn with_preferences(mut self, preferences: &TargetPreferences) -> Self {
        for target in ProxyTarget::ALL {
            if let Some(enabled) = preferences.override_for(target) {
                self.set(target, enabled && target.can_auto_configure());
            }
        }
        self
    }

    pub fn enabled(&self, target: ProxyTarget) -> bool {
        match target {
            ProxyTarget::Codex => self.codex,
            ProxyTarget::GitHubCopilotApp => self.github_copilot_app,
            ProxyTarget::JetBrains => self.jetbrains,
            ProxyTarget::Antigravity => self.antigravity,
            ProxyTarget::Zed => self.zed,
            ProxyTarget::ClaudeCode => self.claude_code,
            ProxyTarget::GrokBuild => self.grok_build,
        }
    }

    pub fn set(&mut self, target: ProxyTarget, enabled: bool) {
        match target {
            ProxyTarget::Codex => self.codex = enabled,
            ProxyTarget::GitHubCopilotApp => self.github_copilot_app = enabled,
            ProxyTarget::JetBrains => self.jetbrains = enabled,
            ProxyTarget::Antigravity => self.antigravity = enabled,
            ProxyTarget::Zed => self.zed = enabled,
            ProxyTarget::ClaudeCode => self.claude_code = enabled,
            ProxyTarget::GrokBuild => self.grok_build = enabled,
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.codex {
            names.push("Codex");
        }
        if self.github_copilot_app {
            names.push("GitHub Copilot App");
        }
        if self.zed {
            names.push("Zed");
        }
        if self.jetbrains {
            names.push("JetBrains");
        }
        if self.antigravity {
            names.push("Antigravity");
        }
        if self.claude_code {
            names.push("Claude Code");
        }
        if self.grok_build {
            names.push("Grok Build");
        }
        names
    }
}

pub fn configure_detected(registry: &Registry, base_url: &str, targets: &DesktopTargets) {
    if targets.codex {
        let _ = codex::install(registry, base_url);
    }
    if targets.github_copilot_app {
        let _ = copilot_app::install(registry, base_url);
    }
    if targets.zed {
        let _ = zed::install(registry, base_url);
    }
    if targets.grok_build {
        let _ = grok::install(registry, base_url);
    }
    if targets.claude_code {
        let _ = claude::install(registry, base_url);
    }
    if targets.antigravity {
        let _ = antigravity::install(base_url);
    }
}

pub fn configure_target(
    registry: &Registry,
    base_url: &str,
    target: ProxyTarget,
    enabled: bool,
) -> anyhow::Result<()> {
    match (target, enabled) {
        (ProxyTarget::Codex, true) => codex::install(registry, base_url).map(|_| ()),
        (ProxyTarget::Codex, false) => codex::uninstall(),
        (ProxyTarget::GitHubCopilotApp, true) => copilot_app::install(registry, base_url),
        (ProxyTarget::GitHubCopilotApp, false) => copilot_app::uninstall(),
        (ProxyTarget::Zed, true) => zed::install(registry, base_url).map(|_| ()),
        (ProxyTarget::Zed, false) => zed::uninstall(),
        (ProxyTarget::GrokBuild, true) => grok::install(registry, base_url).map(|_| ()),
        (ProxyTarget::GrokBuild, false) => grok::uninstall(),
        (ProxyTarget::ClaudeCode, true) => claude::install(registry, base_url).map(|_| ()),
        (ProxyTarget::ClaudeCode, false) => claude::uninstall(),
        // JetBrains stores the credential in its managed credential store.
        (ProxyTarget::JetBrains, _) => Ok(()),
        (ProxyTarget::Antigravity, true) => antigravity::install(base_url).map(|_| ()),
        (ProxyTarget::Antigravity, false) => antigravity::restore(),
    }
}

fn command_exists(command: &str) -> bool {
    env::var_os("PATH").is_some_and(|paths| {
        env::split_paths(&paths).any(|directory| {
            executable_candidates(command)
                .iter()
                .any(|candidate| directory.join(candidate).is_file())
        })
    })
}

fn executable_candidates(command: &str) -> Vec<String> {
    #[cfg(windows)]
    {
        vec![
            format!("{command}.exe"),
            format!("{command}.cmd"),
            format!("{command}.bat"),
            command.to_owned(),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![command.to_owned()]
    }
}

fn application_exists(names: &[&str]) -> bool {
    application_roots()
        .iter()
        .any(|root| names.iter().any(|name| root.join(name).exists()))
}

#[cfg(target_os = "macos")]
fn application_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from("/Applications")];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Applications"));
    }
    roots
}

#[cfg(not(target_os = "macos"))]
fn application_roots() -> Vec<PathBuf> {
    Vec::new()
}

fn jetbrains_installed() -> bool {
    const COMMANDS: &[&str] = &[
        "idea",
        "webstorm",
        "pycharm",
        "goland",
        "rustrover",
        "clion",
        "rider",
        "phpstorm",
        "rubymine",
        "datagrip",
        "studio",
    ];
    const APPLICATIONS: &[&str] = &[
        "IntelliJ IDEA.app",
        "IntelliJ IDEA CE.app",
        "WebStorm.app",
        "PyCharm.app",
        "PyCharm CE.app",
        "GoLand.app",
        "RustRover.app",
        "CLion.app",
        "Rider.app",
        "PhpStorm.app",
        "RubyMine.app",
        "DataGrip.app",
        "Android Studio.app",
    ];

    if COMMANDS.iter().any(|command| command_exists(command)) || application_exists(APPLICATIONS) {
        return true;
    }

    #[cfg(target_os = "windows")]
    {
        let roots = [
            env::var_os("ProgramFiles").map(PathBuf::from),
            env::var_os("LOCALAPPDATA").map(PathBuf::from),
        ];
        if roots.into_iter().flatten().any(|root| {
            root.join("JetBrains").exists()
                || root.join("Programs/JetBrains").exists()
                || root.join("Google/Android Studio").exists()
        }) {
            return true;
        }
    }

    #[cfg(target_os = "linux")]
    {
        if dirs::home_dir().is_some_and(|home| {
            home.join(".local/share/JetBrains/Toolbox/apps").exists()
                || home.join(".local/share/Google/AndroidStudio").exists()
        }) || PathBuf::from("/opt/jetbrains").exists()
            || PathBuf::from("/opt/android-studio").exists()
        {
            return true;
        }
    }

    // JetBrains Toolbox exposes installed products through its command on some
    // platforms. Treat a successful invocation as detection, but never require it.
    Command::new("jetbrains-toolbox")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_names_are_stable_and_ordered() {
        let targets = DesktopTargets {
            codex: true,
            github_copilot_app: true,
            zed: true,
            jetbrains: true,
            antigravity: true,
            claude_code: true,
            grok_build: true,
        };
        assert_eq!(
            targets.names(),
            vec![
                "Codex",
                "GitHub Copilot App",
                "Zed",
                "JetBrains",
                "Antigravity",
                "Claude Code",
                "Grok Build"
            ]
        );
    }

    #[test]
    fn all_supported_includes_antigravity_patch_target() {
        let targets = DesktopTargets::all_supported();
        assert!(targets.codex && targets.github_copilot_app && targets.zed && targets.jetbrains);
        assert!(targets.antigravity);
        assert!(targets.claude_code && targets.grok_build);
    }

    #[test]
    fn saved_antigravity_override_enables_patch_target() {
        let mut preferences = TargetPreferences::default();
        preferences.proxy_to.insert(ProxyTarget::Antigravity, true);

        let targets = DesktopTargets::default().with_preferences(&preferences);

        assert!(targets.antigravity);
    }
}
