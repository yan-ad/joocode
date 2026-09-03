use std::{
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

use anyhow::{Context, bail};
use flate2::read::GzDecoder;
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const DEFAULT_REPOSITORY: &str = "yan-ad/joocode";

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
}

#[cfg(target_os = "windows")]
fn stage_windows_upgrade(
    executable: &Path,
    archive: &[u8],
    target: &str,
    relaunch: bool,
) -> anyhow::Result<()> {
    let install_dir = executable
        .parent()
        .context("the running executable has no install directory")?;
    let probe = install_dir.join(format!(".joocode-write-test-{}", std::process::id()));
    fs::write(&probe, b"").with_context(|| format!("{} is not writable", install_dir.display()))?;
    fs::remove_file(&probe).context("failed to clear the upgrade write test")?;

    let staging = std::env::temp_dir().join(format!(
        "joocode-upgrade-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&staging).context("failed to create Windows upgrade staging")?;
    let archive_path = staging.join(format!("joocode-{target}.zip"));
    fs::write(&archive_path, archive).context("failed to stage the Windows release archive")?;

    let script_path = staging.join("complete-upgrade.ps1");
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    let script = WindowsUpgradePlan {
        parent_pid: std::process::id(),
        target,
        staging: &staging,
        archive: &archive_path,
        install_dir,
        current_executable: executable,
        relaunch,
        relaunch_args: &arguments,
    }
    .render();
    fs::write(&script_path, script).context("failed to write the Windows upgrade helper")?;

    let status = std::process::Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&script_path)
        .spawn()
        .context("failed to launch the Windows PowerShell upgrade helper")?;
    drop(status);
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
struct WindowsUpgradePlan<'a> {
    parent_pid: u32,
    target: &'a str,
    staging: &'a Path,
    archive: &'a Path,
    install_dir: &'a Path,
    current_executable: &'a Path,
    relaunch: bool,
    relaunch_args: &'a [std::ffi::OsString],
}

#[cfg(any(target_os = "windows", test))]
impl WindowsUpgradePlan<'_> {
    fn render(&self) -> String {
        let extracted = self.staging.join("extracted");
        let executable_name = self
            .current_executable
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("jcx.exe");
        let relaunch_path = self.install_dir.join(executable_name);
        let arguments = windows_command_line(self.relaunch_args);
        format!(
            r#"$ErrorActionPreference = 'Stop'
$parentPid = {parent_pid}
$deadline = (Get-Date).AddSeconds(60)
while (Get-Process -Id $parentPid -ErrorAction SilentlyContinue) {{
  if ((Get-Date) -ge $deadline) {{ throw 'Timed out waiting for Joocode to exit' }}
  Start-Sleep -Milliseconds 100
}}

$managedExecutables = @(
  (Join-Path {install_dir} 'jcx.exe'),
  (Join-Path {install_dir} 'joocode.exe')
)
$runtimeDirectory = Join-Path $env:LOCALAPPDATA 'joocode'
$pauseMarker = Join-Path $runtimeDirectory 'Joocode.paused'
$runtimeEntry = Join-Path $runtimeDirectory 'Joocode.cmd'
New-Item -ItemType Directory -Path $runtimeDirectory -Force | Out-Null
Set-Content -LiteralPath $pauseMarker -Value 'paused' -Force
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
  Where-Object {{ $_.CommandLine -like '*Joocode.cmd*' }} |
  ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}
Get-CimInstance Win32_Process -ErrorAction SilentlyContinue |
  Where-Object {{ $_.ProcessId -ne $PID -and $managedExecutables -contains $_.ExecutablePath }} |
  ForEach-Object {{ Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }}

Expand-Archive -LiteralPath {archive} -DestinationPath {extracted} -Force
$releaseRoot = Join-Path {extracted} 'joocode-{target}'
Copy-Item -LiteralPath (Join-Path $releaseRoot 'jcx.exe') -Destination (Join-Path {install_dir} 'jcx.exe') -Force
Copy-Item -LiteralPath (Join-Path $releaseRoot 'joocode.exe') -Destination (Join-Path {install_dir} 'joocode.exe') -Force

if ({relaunch}) {{
  $process = New-Object System.Diagnostics.ProcessStartInfo
  $process.FileName = {relaunch_path}
  $process.Arguments = {arguments}
  $process.WorkingDirectory = {working_directory}
  $process.UseShellExecute = $true
  [System.Diagnostics.Process]::Start($process) | Out-Null
}} elseif (Test-Path -LiteralPath $runtimeEntry) {{
  Remove-Item -LiteralPath $pauseMarker -Force -ErrorAction SilentlyContinue
  Start-Process -FilePath $runtimeEntry -WindowStyle Hidden
}} else {{
  Remove-Item -LiteralPath $pauseMarker -Force -ErrorAction SilentlyContinue
}}

Remove-Item -LiteralPath {staging} -Recurse -Force -ErrorAction SilentlyContinue
"#,
            parent_pid = self.parent_pid,
            archive = powershell_literal(self.archive.to_string_lossy().as_ref()),
            extracted = powershell_literal(extracted.to_string_lossy().as_ref()),
            target = self.target,
            install_dir = powershell_literal(self.install_dir.to_string_lossy().as_ref()),
            relaunch = if self.relaunch { "$true" } else { "$false" },
            relaunch_path = powershell_literal(relaunch_path.to_string_lossy().as_ref()),
            arguments = powershell_literal(&arguments),
            working_directory = powershell_literal(
                std::env::current_dir()
                    .unwrap_or_else(|_| self.install_dir.to_path_buf())
                    .to_string_lossy()
                    .as_ref(),
            ),
            staging = powershell_literal(self.staging.to_string_lossy().as_ref()),
        )
    }
}

#[cfg(any(target_os = "windows", test))]
fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(any(target_os = "windows", test))]
fn windows_command_line(arguments: &[std::ffi::OsString]) -> String {
    arguments
        .iter()
        .map(|argument| windows_quote_argument(&argument.to_string_lossy()))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(any(target_os = "windows", test))]
fn windows_quote_argument(value: &str) -> String {
    if !value.is_empty()
        && !value
            .chars()
            .any(|character| character.is_whitespace() || character == '"')
    {
        return value.to_owned();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0;
    for character in value.chars() {
        if character == '\\' {
            backslashes += 1;
        } else if character == '"' {
            quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
            quoted.push('"');
            backslashes = 0;
        } else {
            quoted.push_str(&"\\".repeat(backslashes));
            backslashes = 0;
            quoted.push(character);
        }
    }
    quoted.push_str(&"\\".repeat(backslashes * 2));
    quoted.push('"');
    quoted
}

pub async fn run(version: Option<&str>) -> anyhow::Result<()> {
    let tag = match version {
        Some(version) => normalize_tag(version),
        None => match check().await? {
            Some(tag) => tag,
            None => {
                println!(
                    "Joocode {} is already up to date",
                    normalize_tag(env!("CARGO_PKG_VERSION"))
                );
                return Ok(());
            }
        },
    };

    if tag == normalize_tag(env!("CARGO_PKG_VERSION")) {
        println!("Joocode {tag} is already up to date");
        return Ok(());
    }

    let executable = install(&tag).await?;
    #[cfg(target_os = "windows")]
    println!(
        "Joocode {tag} upgrade staged for {}; it will finish after this process exits",
        executable.display()
    );
    #[cfg(not(target_os = "windows"))]
    println!("Joocode {tag} installed at {}", executable.display());
    Ok(())
}

pub async fn check() -> anyhow::Result<Option<String>> {
    if !self_upgrade_supported() {
        return Ok(None);
    }

    let repository = repository();
    let release = client()?
        .get(format!(
            "https://api.github.com/repos/{repository}/releases/latest"
        ))
        .send()
        .await
        .context("failed to query the latest Joocode release")?
        .error_for_status()
        .context("GitHub returned an error while checking for updates")?
        .json::<LatestRelease>()
        .await
        .context("failed to parse the latest Joocode release")?;

    if is_newer(&release.tag_name, env!("CARGO_PKG_VERSION"))? {
        Ok(Some(release.tag_name))
    } else {
        Ok(None)
    }
}

pub async fn install(tag: &str) -> anyhow::Result<PathBuf> {
    install_inner(tag, false).await
}

pub async fn install_for_restart(tag: &str) -> anyhow::Result<PathBuf> {
    install_inner(tag, true).await
}

async fn install_inner(tag: &str, relaunch: bool) -> anyhow::Result<PathBuf> {
    #[cfg(not(target_os = "windows"))]
    let _ = relaunch;
    let repository = repository();
    let client = client()?;
    let tag = normalize_tag(tag);

    let target = release_target()?;
    let extension = if cfg!(target_os = "windows") {
        "zip"
    } else {
        "tar.gz"
    };
    let asset = format!("joocode-{target}.{extension}");
    let base_url = format!("https://github.com/{repository}/releases/download/{tag}");
    let archive = client
        .get(format!("{base_url}/{asset}"))
        .send()
        .await
        .with_context(|| format!("failed to download Joocode {tag}"))?
        .error_for_status()
        .with_context(|| format!("release asset not found: {asset}"))?
        .bytes()
        .await
        .context("failed to read the release archive")?;
    let checksums = client
        .get(format!("{base_url}/SHA256SUMS"))
        .send()
        .await
        .context("failed to download release checksums")?
        .error_for_status()
        .context("GitHub returned an error while downloading checksums")?
        .text()
        .await
        .context("failed to read release checksums")?;

    let expected = checksum_for(&checksums, &asset)
        .with_context(|| format!("checksum for {asset} is missing"))?;
    let actual = sha256_hex(&archive);
    if actual != expected {
        bail!("checksum verification failed for {asset}");
    }

    let executable = std::env::current_exe().context("failed to locate the running binary")?;
    #[cfg(target_os = "windows")]
    stage_windows_upgrade(&executable, &archive, &target, relaunch)?;
    #[cfg(not(target_os = "windows"))]
    replace_binary(&executable, &archive, &target)?;
    Ok(executable)
}

pub fn restart_current() -> anyhow::Result<()> {
    let executable = std::env::current_exe().context("failed to locate the updated binary")?;
    let mut command = std::process::Command::new(executable);
    command.args(std::env::args_os().skip(1));

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        let error = command.exec();
        Err(error).context("failed to restart the updated Joocode process")
    }

    #[cfg(not(unix))]
    {
        // Windows replacement and relaunch are performed by the staged
        // PowerShell helper after this process releases its executable.
        Ok(())
    }
}

fn repository() -> String {
    std::env::var("JOOCODE_REPOSITORY")
        .or_else(|_| std::env::var("JOC_REPOSITORY"))
        .unwrap_or_else(|_| DEFAULT_REPOSITORY.to_owned())
}

fn client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("joocode/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to create upgrade client")
}

fn self_upgrade_supported() -> bool {
    matches!(std::env::consts::OS, "linux" | "macos" | "windows")
}

fn is_newer(tag: &str, current: &str) -> anyhow::Result<bool> {
    let latest = Version::parse(tag.trim_start_matches('v'))
        .with_context(|| format!("invalid release version: {tag}"))?;
    let current =
        Version::parse(current).with_context(|| format!("invalid current version: {current}"))?;
    Ok(latest > current)
}

fn normalize_tag(version: &str) -> String {
    if version.starts_with('v') {
        version.to_owned()
    } else {
        format!("v{version}")
    }
}

fn release_target() -> anyhow::Result<String> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => bail!("unsupported architecture for self-upgrade: {other}"),
    };
    let os = match std::env::consts::OS {
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        other => bail!("unsupported operating system for self-upgrade: {other}"),
    };
    Ok(format!("{arch}-{os}"))
}

fn checksum_for(checksums: &str, asset: &str) -> Option<String> {
    checksums.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let checksum = fields.next()?;
        let field = fields.next()?;
        let name = field.strip_prefix('*').unwrap_or(field);
        (name == asset).then(|| checksum.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn replace_binary(executable: &Path, archive: &[u8], target: &str) -> anyhow::Result<()> {
    let temp_dir = tempfile_dir(executable)?;
    tar::Archive::new(GzDecoder::new(Cursor::new(archive)))
        .unpack(&temp_dir)
        .context("failed to extract the release archive")?;

    let binary_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| matches!(*name, "jcx" | "joocode"))
        .unwrap_or("jcx");
    let extracted = temp_dir.join(format!("joocode-{target}/{binary_name}"));
    if !extracted.is_file() {
        bail!("release archive does not contain the expected {binary_name} binary");
    }

    replace_executable(executable, &extracted)?;

    let alias_name = if binary_name == "jcx" {
        "joocode"
    } else {
        "jcx"
    };
    let alias_source = temp_dir.join(format!("joocode-{target}/{alias_name}"));
    let alias_destination = executable.with_file_name(alias_name);
    if alias_source.is_file() {
        replace_executable(&alias_destination, &alias_source)
            .with_context(|| format!("failed to install the {alias_name} command"))?;
    }

    let _ = fs::remove_dir_all(temp_dir);
    Ok(())
}

fn replace_executable(destination: &Path, source: &Path) -> anyhow::Result<()> {
    let replacement = destination.with_extension(format!("upgrade-{}", std::process::id()));
    fs::copy(source, &replacement).context("failed to stage the new Joocode binary")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755))
            .context("failed to set executable permissions")?;
    }
    fs::rename(&replacement, destination).with_context(|| {
        format!(
            "failed to replace {}; check that the install directory is writable",
            destination.display()
        )
    })?;
    Ok(())
}

fn tempfile_dir(executable: &Path) -> anyhow::Result<PathBuf> {
    let parent = executable
        .parent()
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| Path::new("/tmp"));
    let path = parent.join(format!(".joocode-upgrade-{}", std::process::id()));
    if path.exists() {
        fs::remove_dir_all(&path).context("failed to clear previous upgrade staging")?;
    }
    fs::create_dir_all(&path).context("failed to create upgrade staging directory")?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Write,
        path::{Path, PathBuf},
    };

    use flate2::{Compression, write::GzEncoder};

    use super::{
        WindowsUpgradePlan, checksum_for, is_newer, normalize_tag, replace_binary,
        windows_quote_argument,
    };

    fn test_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "joocode-upgrade-test-{name}-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("thread")
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn release_archive(target: &str) -> Vec<u8> {
        let encoder = GzEncoder::new(Vec::new(), Compression::default());
        let mut archive = tar::Builder::new(encoder);
        for (name, contents) in [
            ("joocode", b"new-joocode".as_slice()),
            ("jcx", b"new-jcx".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(contents.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            archive
                .append_data(
                    &mut header,
                    Path::new(&format!("joocode-{target}/{name}")),
                    contents,
                )
                .unwrap();
        }
        let encoder = archive.into_inner().unwrap();
        encoder.finish().unwrap()
    }

    #[test]
    fn normalizes_release_tags() {
        assert_eq!(normalize_tag("0.2.0"), "v0.2.0");
        assert_eq!(normalize_tag("v0.2.0"), "v0.2.0");
    }

    #[test]
    fn reads_gnu_and_bsd_checksum_formats() {
        let checksums = "abc  joocode-a.tar.gz\ndef *joocode-b.tar.gz\n";
        assert_eq!(
            checksum_for(checksums, "joocode-a.tar.gz"),
            Some("abc".into())
        );
        assert_eq!(
            checksum_for(checksums, "joocode-b.tar.gz"),
            Some("def".into())
        );
    }

    #[test]
    fn only_reports_strictly_newer_semver_releases() {
        assert!(is_newer("v0.2.0", "0.1.10").unwrap());
        assert!(!is_newer("v0.1.10", "0.1.10").unwrap());
        assert!(!is_newer("v0.1.9", "0.1.10").unwrap());
    }

    #[test]
    fn legacy_joocode_upgrade_installs_missing_jcx_command() {
        let directory = test_dir("legacy-alias");
        let executable = directory.join("joocode");
        let mut file = fs::File::create(&executable).unwrap();
        file.write_all(b"old-joocode").unwrap();

        replace_binary(
            &executable,
            &release_archive("aarch64-apple-darwin"),
            "aarch64-apple-darwin",
        )
        .unwrap();

        assert_eq!(fs::read(&executable).unwrap(), b"new-joocode");
        assert_eq!(fs::read(directory.join("jcx")).unwrap(), b"new-jcx");
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn windows_upgrade_waits_replaces_both_commands_and_can_relaunch() {
        let root = PathBuf::from(r"C:\Users\O'Brien\App Data\joocode-upgrade");
        let install = PathBuf::from(r"C:\Users\O'Brien\.local\bin");
        let archive = root.join("release.zip");
        let executable = install.join("jcx.exe");
        let arguments = ["--port".into(), "10101".into()];
        let script = WindowsUpgradePlan {
            parent_pid: 42,
            target: "x86_64-pc-windows-msvc",
            staging: &root,
            archive: &archive,
            install_dir: &install,
            current_executable: &executable,
            relaunch: true,
            relaunch_args: &arguments,
        }
        .render();

        assert!(script.contains("Get-Process -Id $parentPid"));
        assert!(script.contains("joocode-x86_64-pc-windows-msvc"));
        assert!(script.contains("'C:\\Users\\O''Brien\\.local\\bin'"));
        assert!(script.contains("'jcx.exe'"));
        assert!(script.contains("'joocode.exe'"));
        assert!(script.contains("Joocode.paused"));
        assert!(script.contains("Joocode.cmd"));
        assert!(script.contains("$process.Arguments = '--port 10101'"));
        assert!(script.contains("if ($true)"));
    }

    #[test]
    fn windows_arguments_are_quoted_for_create_process() {
        assert_eq!(windows_quote_argument("plain"), "plain");
        assert_eq!(windows_quote_argument("two words"), r#""two words""#);
        assert_eq!(windows_quote_argument(r#"a"b"#), r#""a\"b""#);
    }
}
