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

pub async fn run(version: Option<&str>) -> anyhow::Result<()> {
    let tag = match version {
        Some(version) => normalize_tag(version),
        None => match check().await? {
            Some(tag) => tag,
            None => {
                println!(
                    "JustOpenCode {} is already up to date",
                    normalize_tag(env!("CARGO_PKG_VERSION"))
                );
                return Ok(());
            }
        },
    };

    if tag == normalize_tag(env!("CARGO_PKG_VERSION")) {
        println!("JustOpenCode {tag} is already up to date");
        return Ok(());
    }

    let executable = install(&tag).await?;
    println!("JustOpenCode {tag} installed at {}", executable.display());
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
        .context("failed to query the latest JustOpenCode release")?
        .error_for_status()
        .context("GitHub returned an error while checking for updates")?
        .json::<LatestRelease>()
        .await
        .context("failed to parse the latest JustOpenCode release")?;

    if is_newer(&release.tag_name, env!("CARGO_PKG_VERSION"))? {
        Ok(Some(release.tag_name))
    } else {
        Ok(None)
    }
}

pub async fn install(tag: &str) -> anyhow::Result<PathBuf> {
    let repository = repository();
    let client = client()?;
    let tag = normalize_tag(tag);

    let target = release_target()?;
    let asset = format!("joocode-{target}.tar.gz");
    let base_url = format!("https://github.com/{repository}/releases/download/{tag}");
    let archive = client
        .get(format!("{base_url}/{asset}"))
        .send()
        .await
        .with_context(|| format!("failed to download JustOpenCode {tag}"))?
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
    replace_binary(&executable, &archive, &target)?;
    Ok(executable)
}

pub fn restart_current() -> anyhow::Result<()> {
    let executable = std::env::current_exe().context("failed to locate the updated binary")?;
    std::process::Command::new(executable)
        .args(std::env::args_os().skip(1))
        .spawn()
        .context("failed to restart the updated JustOpenCode process")?;
    Ok(())
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
    matches!(std::env::consts::OS, "linux" | "macos")
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
        "windows" => {
            bail!("self-upgrade is not supported on Windows; download the latest ZIP release")
        }
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

    let extracted = temp_dir.join(format!("joocode-{target}/joocode"));
    if !extracted.is_file() {
        bail!("release archive does not contain the expected joocode binary");
    }

    let replacement = executable.with_extension(format!("upgrade-{}", std::process::id()));
    fs::copy(&extracted, &replacement).context("failed to stage the new JustOpenCode binary")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&replacement, fs::Permissions::from_mode(0o755))
            .context("failed to set executable permissions")?;
    }
    fs::rename(&replacement, executable).with_context(|| {
        format!(
            "failed to replace {}; check that the install directory is writable",
            executable.display()
        )
    })?;
    let _ = fs::remove_dir_all(temp_dir);
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
    use super::{checksum_for, is_newer, normalize_tag};

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
}
