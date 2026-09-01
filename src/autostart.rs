use std::{fs, path::PathBuf};

use anyhow::Context;

const LABEL: &str = "dev.joocode.proxy";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    On,
    Off,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::On => "On",
            Self::Off => "Off",
        }
    }

    pub fn enabled(self) -> bool {
        matches!(self, Self::On)
    }
}

pub fn status() -> Status {
    if entry_path().is_file() {
        Status::On
    } else {
        Status::Off
    }
}

pub fn toggle() -> anyhow::Result<Status> {
    if status().enabled() {
        disable()?;
        Ok(Status::Off)
    } else {
        enable()?;
        Ok(Status::On)
    }
}

fn executable() -> anyhow::Result<PathBuf> {
    std::env::current_exe().context("failed to locate the Joocode executable")
}

#[cfg(target_os = "macos")]
fn log_dir() -> anyhow::Result<PathBuf> {
    let directory = dirs::data_local_dir()
        .context("could not determine the local data directory")?
        .join("joocode");
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    Ok(directory)
}

#[cfg(target_os = "macos")]
fn entry_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn enable() -> anyhow::Result<()> {
    let path = entry_path();
    let parent = path.parent().context("invalid LaunchAgent path")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let executable = xml_escape(&executable()?.to_string_lossy());
    let logs = log_dir()?;
    let stdout = xml_escape(&logs.join("autostart.log").to_string_lossy());
    let stderr = xml_escape(&logs.join("autostart-error.log").to_string_lossy());
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{executable}</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><false/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{stdout}</string>
  <key>StandardErrorPath</key><string>{stderr}</string>
</dict>
</plist>
"#
    );
    fs::write(&path, plist).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn disable() -> anyhow::Result<()> {
    remove_entry()
}

#[cfg(target_os = "linux")]
fn entry_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("systemd/user")
        .join(format!("{LABEL}.service"))
}

#[cfg(target_os = "linux")]
fn enable() -> anyhow::Result<()> {
    let path = entry_path();
    let parent = path.parent().context("invalid systemd user path")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let executable = systemd_escape(&executable()?.to_string_lossy());
    let service = format!(
        "[Unit]\nDescription=Joocode desktop AI proxy\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={executable}\nRestart=on-failure\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n"
    );
    fs::write(&path, service).with_context(|| format!("failed to write {}", path.display()))?;
    run_systemctl(["daemon-reload"])?;
    run_systemctl([
        "enable",
        path.file_name().unwrap().to_string_lossy().as_ref(),
    ])?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn disable() -> anyhow::Result<()> {
    let path = entry_path();
    let unit = path
        .file_name()
        .context("invalid systemd unit path")?
        .to_string_lossy()
        .into_owned();
    let _ = std::process::Command::new("systemctl")
        .args(["--user", "disable", &unit])
        .output();
    remove_entry()?;
    let _ = run_systemctl(["daemon-reload"]);
    Ok(())
}

#[cfg(target_os = "linux")]
fn run_systemctl<const N: usize>(arguments: [&str; N]) -> anyhow::Result<()> {
    let output = std::process::Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .output()
        .context("failed to run systemctl --user")?;
    if !output.status.success() {
        anyhow::bail!(
            "systemctl --user failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn entry_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Microsoft/Windows/Start Menu/Programs/Startup/Joocode.cmd")
}

#[cfg(target_os = "windows")]
fn enable() -> anyhow::Result<()> {
    let path = entry_path();
    let parent = path.parent().context("invalid Windows Startup path")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let executable = executable()?;
    let command = format!(
        "@echo off\r\nstart \"\" /min \"{}\"\r\n",
        executable.display()
    );
    fs::write(&path, command).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn disable() -> anyhow::Result<()> {
    remove_entry()
}

fn remove_entry() -> anyhow::Result<()> {
    let path = entry_path();
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

#[cfg(target_os = "macos")]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(target_os = "linux")]
fn systemd_escape(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_labels_are_stable() {
        assert_eq!(Status::On.label(), "On");
        assert_eq!(Status::Off.label(), "Off");
        assert!(Status::On.enabled());
        assert!(!Status::Off.enabled());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn xml_values_are_escaped() {
        assert_eq!(xml_escape("a&<\"'"), "a&amp;&lt;&quot;&apos;");
    }
}
