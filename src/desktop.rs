use std::{env, path::PathBuf, process::Command};

use crate::{codex, provider::Registry, zed};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DesktopTargets {
    pub codex: bool,
    pub zed: bool,
    pub jetbrains: bool,
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
        Self {
            codex: command_exists("codex") || application_exists(&["Codex.app"]),
            zed: zed_installed(),
            jetbrains: jetbrains_installed(),
        }
    }

    pub fn all_supported() -> Self {
        Self {
            codex: true,
            zed: true,
            jetbrains: true,
        }
    }

    pub fn names(&self) -> Vec<&'static str> {
        let mut names = Vec::new();
        if self.codex {
            names.push("Codex");
        }
        if self.zed {
            names.push("Zed");
        }
        if self.jetbrains {
            names.push("JetBrains");
        }
        names
    }
}

pub fn configure_detected(registry: &Registry, base_url: &str, targets: &DesktopTargets) {
    if targets.codex {
        let _ = codex::install(registry, base_url);
    }
    if targets.zed {
        let _ = zed::install(registry, base_url);
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
            zed: true,
            jetbrains: true,
        };
        assert_eq!(targets.names(), vec!["Codex", "Zed", "JetBrains"]);
    }

    #[test]
    fn all_supported_enables_every_target() {
        let targets = DesktopTargets::all_supported();
        assert!(targets.codex && targets.zed && targets.jetbrains);
    }
}
