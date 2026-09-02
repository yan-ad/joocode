use std::{fs, path::PathBuf};

use anyhow::Context;

const LABEL: &str = "dev.joocode.proxy";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    On,
    Off,
}

#[cfg(target_os = "macos")]
fn launchctl_service_loaded(service: &str) -> anyhow::Result<bool> {
    let output = std::process::Command::new("/bin/launchctl")
        .args(["print", service])
        .output()
        .context("failed to inspect the Joocode background service")?;
    if output.status.success() {
        return Ok(true);
    }

    if launchctl_item_not_found(&output.stderr) {
        return Ok(false);
    }
    anyhow::bail!(
        "failed to inspect the Joocode background service: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )
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

/// Pause the background proxy before the interactive dashboard binds its port.
/// The runtime service definition is refreshed independently of the Auto-start
/// preference, so every dashboard session can hand the proxy back on exit.
pub fn prepare_dashboard_handoff() -> anyhow::Result<()> {
    let _ = pause_platform();
    ensure_runtime_service()?;
    Ok(())
}

/// Toggle whether Joocode starts automatically after login/device restart.
/// This intentionally does not start or stop the current background session.
pub fn toggle_for_dashboard() -> anyhow::Result<Status> {
    if status().enabled() {
        disable_boot()?;
        Ok(Status::Off)
    } else {
        enable_boot()?;
        Ok(Status::On)
    }
}

/// Start the background proxy now without changing the Auto-start preference.
pub fn start() -> anyhow::Result<()> {
    ensure_runtime_service()?;
    resume_platform()
}

/// Stop the currently running background proxy without changing Auto-start.
pub fn stop() -> anyhow::Result<()> {
    pause_platform()
}

/// Resume the persistent proxy without holding the interactive terminal open.
/// The child owns the service-manager wait while the dashboard process can
/// restore the terminal and exit immediately.
pub fn resume_detached() -> anyhow::Result<()> {
    let executable = executable()?;
    let mut command = std::process::Command::new(executable);
    command
        .arg("start")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    command
        .spawn()
        .context("failed to hand the Joocode proxy back to the background")?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub fn status() -> Status {
    if boot_entry_path().is_file() {
        Status::On
    } else {
        Status::Off
    }
}

#[cfg(target_os = "linux")]
pub fn status() -> Status {
    let unit = format!("{LABEL}.service");
    let enabled = std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", &unit])
        .status()
        .is_ok_and(|status| status.success());
    if enabled { Status::On } else { Status::Off }
}

#[cfg(target_os = "windows")]
pub fn status() -> Status {
    if boot_entry_path().is_file() {
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
fn data_dir() -> anyhow::Result<PathBuf> {
    let directory = dirs::data_local_dir()
        .context("could not determine the local data directory")?
        .join("joocode");
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    Ok(directory)
}

#[cfg(target_os = "macos")]
fn runtime_entry_path() -> anyhow::Result<PathBuf> {
    Ok(data_dir()?.join(format!("{LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn boot_entry_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

#[cfg(target_os = "macos")]
fn macos_plist(executable: &str, stdout: &str, stderr: &str) -> String {
    format!(
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
    )
}

#[cfg(target_os = "macos")]
fn service_contents() -> anyhow::Result<String> {
    let executable = xml_escape(&executable()?.to_string_lossy());
    let logs = data_dir()?;
    let stdout = xml_escape(&logs.join("autostart.log").to_string_lossy());
    let stderr = xml_escape(&logs.join("autostart-error.log").to_string_lossy());
    Ok(macos_plist(&executable, &stdout, &stderr))
}

#[cfg(target_os = "macos")]
fn ensure_runtime_service() -> anyhow::Result<()> {
    let path = runtime_entry_path()?;
    fs::write(&path, service_contents()?)
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(target_os = "macos")]
fn enable_boot() -> anyhow::Result<()> {
    ensure_runtime_service()?;
    let path = boot_entry_path();
    let parent = path.parent().context("invalid LaunchAgent path")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    fs::write(&path, service_contents()?)
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(target_os = "macos")]
fn disable_boot() -> anyhow::Result<()> {
    remove_file(boot_entry_path())
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
    let service = format!("{}/{LABEL}", launchctl_domain()?);
    let output = std::process::Command::new("/bin/launchctl")
        .args(["bootout", &service])
        .output()
        .context("failed to stop the Joocode background service")?;
    if output.status.success() || launchctl_item_not_found(&output.stderr) {
        Ok(())
    } else {
        anyhow::bail!(
            "failed to stop the Joocode background service: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

#[cfg(target_os = "macos")]
fn resume_platform() -> anyhow::Result<()> {
    let domain = launchctl_domain()?;
    let service = format!("{domain}/{LABEL}");

    // `launchctl bootstrap` is not idempotent and may report the opaque
    // "Bootstrap failed: 5: Input/output error" when the job is already
    // registered. Query the service first so `jcx start` is safe to run even
    // when the persistent proxy is already healthy.
    if launchctl_service_loaded(&service)? {
        return Ok(());
    }

    let runtime_path = runtime_entry_path()?;
    let output = std::process::Command::new("/bin/launchctl")
        .args(["bootstrap", &domain])
        .arg(&runtime_path)
        .output()
        .context("failed to start the Joocode background service")?;
    if output.status.success() {
        // The service has RunAtLoad=true, so a successful bootstrap starts it.
        // Avoid `kickstart -k`: it synchronously tears down and relaunches the
        // process, adding several seconds to dashboard shutdown.
        return Ok(());
    }
    // macOS can return a generic I/O error for a stale/already-loaded job.
    // Trust the authoritative service query instead of matching only stderr.
    if launchctl_service_loaded(&service)? {
        return Ok(());
    }
    if !launchctl_already_loaded(&output.stderr) {
        anyhow::bail!(
            "failed to start the Joocode background service: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    // Already loaded usually means the KeepAlive service is running. A plain
    // kickstart is a cheap idempotent nudge and does not kill a healthy daemon.
    let output = std::process::Command::new("/bin/launchctl")
        .args(["kickstart", &service])
        .output()
        .context("failed to kick-start the Joocode background service")?;
    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "failed to kick-start the Joocode background service: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

#[cfg(target_os = "macos")]
fn launchctl_item_not_found(stderr: &[u8]) -> bool {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    message.contains("could not find specified service")
        || message.contains("could not find service")
        || message.contains("no such process")
        || message.contains("not found")
}

#[cfg(target_os = "macos")]
fn launchctl_already_loaded(stderr: &[u8]) -> bool {
    let message = String::from_utf8_lossy(stderr).to_ascii_lowercase();
    message.contains("service already loaded") || message.contains("already exists")
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
fn runtime_entry_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("systemd/user")
        .join(format!("{LABEL}.service"))
}

#[cfg(target_os = "linux")]
fn systemd_service(executable: &str) -> String {
    format!(
        "[Unit]\nDescription=Joocode desktop AI proxy\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nExecStart={executable} serve --host 127.0.0.1 --port 10100\nRestart=always\nRestartSec=3\n\n[Install]\nWantedBy=default.target\n"
    )
}

#[cfg(target_os = "linux")]
fn ensure_runtime_service() -> anyhow::Result<()> {
    let path = runtime_entry_path();
    let parent = path.parent().context("invalid systemd user path")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let executable = systemd_escape(&executable()?.to_string_lossy());
    fs::write(&path, systemd_service(&executable))
        .with_context(|| format!("failed to write {}", path.display()))?;
    run_systemctl(["daemon-reload"])
}

#[cfg(target_os = "linux")]
fn enable_boot() -> anyhow::Result<()> {
    ensure_runtime_service()?;
    let unit = format!("{LABEL}.service");
    run_systemctl(["enable", &unit])
}

#[cfg(target_os = "linux")]
fn disable_boot() -> anyhow::Result<()> {
    let unit = format!("{LABEL}.service");
    run_systemctl(["disable", &unit])
}

#[cfg(target_os = "linux")]
fn pause_platform() -> anyhow::Result<()> {
    let unit = format!("{LABEL}.service");
    run_systemctl(["stop", &unit])
}

#[cfg(target_os = "linux")]
fn resume_platform() -> anyhow::Result<()> {
    let unit = format!("{LABEL}.service");
    run_systemctl(["start", &unit])
}

#[cfg(target_os = "linux")]
fn run_systemctl<const N: usize>(arguments: [&str; N]) -> anyhow::Result<()> {
    let output = std::process::Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .output()
        .context("failed to run systemctl --user")?;
    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!(
            "systemctl --user failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
}

#[cfg(target_os = "linux")]
fn systemd_escape(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "windows")]
fn runtime_dir() -> anyhow::Result<PathBuf> {
    let directory = dirs::data_local_dir()
        .context("could not determine the local data directory")?
        .join("joocode");
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create {}", directory.display()))?;
    Ok(directory)
}

#[cfg(target_os = "windows")]
fn runtime_entry_path() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("Joocode.cmd"))
}

#[cfg(target_os = "windows")]
fn boot_entry_path() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Microsoft/Windows/Start Menu/Programs/Startup/Joocode.cmd")
}

#[cfg(target_os = "windows")]
fn pause_marker() -> anyhow::Result<PathBuf> {
    Ok(runtime_dir()?.join("Joocode.paused"))
}

#[cfg(target_os = "windows")]
fn supervisor_script() -> anyhow::Result<String> {
    Ok(format!(
        "@echo off\r\n:joocode_loop\r\nif exist \"{}\" goto joocode_wait\r\n\"{}\" serve --host 127.0.0.1 --port 10100\r\n:joocode_wait\r\ntimeout /t 3 /nobreak >nul\r\ngoto joocode_loop\r\n",
        pause_marker()?.display(),
        executable()?.display()
    ))
}

#[cfg(target_os = "windows")]
fn ensure_runtime_service() -> anyhow::Result<()> {
    let path = runtime_entry_path()?;
    fs::write(&path, supervisor_script()?)
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(target_os = "windows")]
fn enable_boot() -> anyhow::Result<()> {
    ensure_runtime_service()?;
    let path = boot_entry_path();
    let parent = path.parent().context("invalid Windows Startup path")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let pause_marker = pause_marker()?;
    fs::write(
        &path,
        format!(
            "@echo off\r\ndel /Q \"{}\" 2>nul\r\nstart \"\" /MIN \"{}\"\r\n",
            pause_marker.display(),
            runtime_entry_path()?.display()
        ),
    )
    .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(target_os = "windows")]
fn disable_boot() -> anyhow::Result<()> {
    remove_file(boot_entry_path())
}

#[cfg(target_os = "windows")]
fn pause_platform() -> anyhow::Result<()> {
    fs::write(pause_marker()?, b"paused")
        .context("failed to create the Joocode background pause marker")?;
    let script = "Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like '*Joocode.cmd*' -or $_.CommandLine -like '* serve --host 127.0.0.1 --port 10100*' } | Invoke-CimMethod -MethodName Terminate | Out-Null";
    let _ = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output();
    Ok(())
}

#[cfg(target_os = "windows")]
fn resume_platform() -> anyhow::Result<()> {
    match fs::remove_file(pause_marker()?) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("failed to clear the Joocode pause marker"),
    }
    std::process::Command::new("cmd")
        .args(["/C", "start", "", "/MIN"])
        .arg(runtime_entry_path()?)
        .spawn()
        .context("failed to start the Joocode background supervisor")?;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn remove_file(path: PathBuf) -> anyhow::Result<()> {
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
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

    #[cfg(target_os = "macos")]
    #[test]
    fn launch_agent_is_persistent_and_headless() {
        let plist = macos_plist("/tmp/jcx", "/tmp/out", "/tmp/err");
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
        assert!(plist.contains("<string>serve</string>"));
        assert!(plist.contains("<string>10100</string>"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn runtime_service_is_separate_from_boot_registration() {
        let runtime = runtime_entry_path().unwrap();
        let boot = boot_entry_path();
        assert_ne!(runtime, boot);
        assert!(boot.ends_with("Library/LaunchAgents/dev.joocode.proxy.plist"));
        assert!(runtime.ends_with("joocode/dev.joocode.proxy.plist"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn recognizes_launchctl_service_not_loaded_message() {
        assert!(launchctl_item_not_found(
            b"Bad request.\nCould not find service \"dev.joocode.proxy\" in domain for user gui: 501"
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn systemd_service_is_persistent_and_headless() {
        let service = systemd_service("/tmp/jcx");
        assert!(service.contains("ExecStart=/tmp/jcx serve"));
        assert!(service.contains("Restart=always"));
    }
}
