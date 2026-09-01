use std::{fs, path::PathBuf};

use anyhow::Context;

const LABEL: &str = "dev.joocode.proxy";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    On,
    Off,
}

/// Pause the background proxy and refresh an existing startup entry to the
/// current persistent-service definition. This transparently migrates older
/// login-only entries when the dashboard is opened after an upgrade.
pub fn prepare_dashboard_handoff() -> anyhow::Result<()> {
    if !status().enabled() {
        return Ok(());
    }
    pause_platform()?;
    enable()?;
    pause_platform()?;
    Ok(())
}

/// Enable or disable Auto-start without changing the currently running
/// dashboard process. Enabling writes and registers the persistent service,
/// then leaves it paused until the dashboard hands the port back on exit.
pub fn toggle_for_dashboard() -> anyhow::Result<Status> {
    if status().enabled() {
        disable()?;
        Ok(Status::Off)
    } else {
        enable()?;
        pause_platform()?;
        Ok(Status::On)
    }
}

#[cfg(target_os = "linux")]
fn pause_platform() -> anyhow::Result<()> {
    let unit = format!("{LABEL}.service");
    run_systemctl(["stop", unit.as_str()])
}

#[cfg(target_os = "linux")]
fn resume_platform() -> anyhow::Result<()> {
    let unit = format!("{LABEL}.service");
    run_systemctl(["start", unit.as_str()])
}

#[cfg(target_os = "windows")]
fn pause_marker() -> PathBuf {
    entry_path().with_extension("paused")
}

/// Start the persistent proxy when Auto-start is enabled.
pub fn resume() -> anyhow::Result<()> {
    if status().enabled() {
        resume_platform()?;
    }
    Ok(())
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

fn executable() -> anyhow::Result<PathBuf> {
    let current = std::env::current_exe().context("failed to locate the Joocode executable")?;
    if current.to_string_lossy().contains("/Cellar/") {
        for candidate in stable_executable_candidates() {
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Ok(current)
}

#[cfg(unix)]
fn stable_executable_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/opt/homebrew/bin/jcx"),
        PathBuf::from("/usr/local/bin/jcx"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin/jcx"),
        PathBuf::from("/opt/homebrew/bin/joocode"),
        PathBuf::from("/usr/local/bin/joocode"),
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin/joocode"),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".local/bin/jcx"));
        candidates.push(home.join(".cargo/bin/jcx"));
        candidates.push(home.join(".local/bin/joocode"));
        candidates.push(home.join(".cargo/bin/joocode"));
    }
    candidates
}

#[cfg(windows)]
fn stable_executable_candidates() -> Vec<PathBuf> {
    Vec::new()
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
  <array>
    <string>{executable}</string>
    <string>serve</string>
    <string>--host</string><string>127.0.0.1</string>
    <string>--port</string><string>10100</string>
  </array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>3</integer>
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
    remove_entry()?;
    let _ = pause_platform();
    Ok(())
}

#[cfg(target_os = "macos")]
fn launchctl_domain() -> anyhow::Result<String> {
    let output = std::process::Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .context("failed to determine the current macOS user id")?;
    if !output.status.success() {
        anyhow::bail!("failed to determine the current macOS user id");
    }
    Ok(format!(
        "gui/{}",
        String::from_utf8_lossy(&output.stdout).trim()
    ))
}

#[cfg(target_os = "macos")]
fn pause_platform() -> anyhow::Result<()> {
    let domain = launchctl_domain()?;
    let service = format!("{domain}/{LABEL}");
    let output = std::process::Command::new("/bin/launchctl")
        .args(["bootout", &service])
        .output()
        .context("failed to stop the Joocode LaunchAgent")?;
    if output.status.success() || launchctl_item_not_found(&output.stderr) {
        Ok(())
    } else {
        anyhow::bail!(
            "failed to stop the Joocode LaunchAgent: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

#[cfg(target_os = "macos")]
fn resume_platform() -> anyhow::Result<()> {
    let domain = launchctl_domain()?;
    let output = std::process::Command::new("/bin/launchctl")
        .args(["bootstrap", &domain])
        .arg(entry_path())
        .output()
        .context("failed to start the Joocode LaunchAgent")?;
    if !output.status.success() && !launchctl_already_loaded(&output.stderr) {
        anyhow::bail!(
            "failed to start the Joocode LaunchAgent: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let service = format!("{domain}/{LABEL}");
    let output = std::process::Command::new("/bin/launchctl")
        .args(["kickstart", "-k", &service])
        .output()
        .context("failed to kick-start the Joocode LaunchAgent")?;
    if !output.status.success() {
        anyhow::bail!(
            "failed to kick-start the Joocode LaunchAgent: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn launchctl_item_not_found(stderr: &[u8]) -> bool {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    message.contains("could not find specified service")
        || message.contains("no such process")
        || message.contains("not found")
}

#[cfg(target_os = "macos")]
fn launchctl_already_loaded(stderr: &[u8]) -> bool {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    message.contains("service already loaded") || message.contains("already exists")
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
        "[Unit]\nDescription=Joocode desktop AI proxy\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={executable} serve --host 127.0.0.1 --port 10100\nRestart=always\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n"
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
        .args(["--user", "disable", "--now", &unit])
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
        "@echo off\r\n:joocode_loop\r\nif exist \"{}\" goto joocode_wait\r\n\"{}\" serve --host 127.0.0.1 --port 10100\r\n:joocode_wait\r\ntimeout /t 3 /nobreak >nul\r\nif exist \"{}\" goto joocode_loop\r\n",
        pause_marker().display(),
        executable.display(),
        path.display()
    );
    fs::write(&path, command).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn disable() -> anyhow::Result<()> {
    remove_entry()?;
    let _ = pause_platform();
    let _ = fs::remove_file(pause_marker());
    Ok(())
}

#[cfg(target_os = "windows")]
fn pause_platform() -> anyhow::Result<()> {
    fs::write(pause_marker(), b"paused")
        .context("failed to create the Joocode background pause marker")?;
    let _ = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '* serve --host 127.0.0.1 --port 10100*' } | Invoke-CimMethod -MethodName Terminate | Out-Null",
        ])
        .output();
    Ok(())
}

#[cfg(target_os = "windows")]
fn resume_platform() -> anyhow::Result<()> {
    match fs::remove_file(pause_marker()) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to clear the Joocode pause marker"),
    }
    std::process::Command::new("cmd")
        .args(["/C", "start", "", "/MIN"])
        .arg(entry_path())
        .spawn()
        .context("failed to start the Joocode background supervisor")?;
    Ok(())
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
