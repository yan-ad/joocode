use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::Context;

const SECURITY: &str = "/usr/bin/security";

pub fn find_generic_password(service: &str, account: Option<&str>) -> Option<String> {
    let mut args = vec!["find-generic-password", "-s", service];
    if let Some(account) = account {
        args.extend(["-a", account]);
    }
    args.push("-w");

    let output = run_with_user_keychain(&args).ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

pub fn ensure_internet_password(account: &str, server: &str, password: &str) -> anyhow::Result<()> {
    let lookup = run_with_user_keychain(&["find-internet-password", "-a", account, "-s", server])?;
    if lookup.status.success() {
        return Ok(());
    }
    if !is_missing_item(&lookup) && !is_missing_keychain(&lookup) {
        return Err(keychain_error("inspect", &lookup));
    }

    let output = run_with_user_keychain(&[
        "add-internet-password",
        "-a",
        account,
        "-s",
        server,
        "-w",
        password,
        "-U",
    ])?;
    if output.status.success() {
        return Ok(());
    }

    Err(keychain_error("store", &output))
}

fn run_with_user_keychain(args: &[&str]) -> anyhow::Result<Output> {
    let keychains = user_keychains();
    if keychains.is_empty() {
        return run_security(args);
    }

    let mut last_missing = None;
    for keychain in keychains {
        let mut command = Command::new(SECURITY);
        command.args(args).arg(&keychain);
        let output = command
            .output()
            .with_context(|| format!("failed to run {SECURITY}"))?;
        if output.status.success() || !is_missing_keychain(&output) {
            return Ok(output);
        }
        last_missing = Some(output);
    }

    Ok(last_missing.expect("user keychain candidates are not empty"))
}

fn run_security(args: &[&str]) -> anyhow::Result<Output> {
    Command::new(SECURITY)
        .args(args)
        .output()
        .with_context(|| format!("failed to run {SECURITY}"))
}

fn user_keychains() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(path) = std::env::var_os("JOOCODE_KEYCHAIN").map(PathBuf::from) {
        push_existing(&mut candidates, path);
    }

    if let Ok(output) = run_security(&["default-keychain", "-d", "user"])
        && output.status.success()
        && let Ok(value) = String::from_utf8(output.stdout)
    {
        push_existing(&mut candidates, PathBuf::from(unquote(value.trim())));
    }

    if let Some(home) = dirs::home_dir() {
        push_existing(
            &mut candidates,
            home.join("Library/Keychains/login.keychain-db"),
        );
        push_existing(
            &mut candidates,
            home.join("Library/Keychains/login.keychain"),
        );
    }

    if let Ok(output) = run_security(&["list-keychains", "-d", "user"])
        && output.status.success()
        && let Ok(value) = String::from_utf8(output.stdout)
    {
        for line in value.lines() {
            push_existing(&mut candidates, PathBuf::from(unquote(line.trim())));
        }
    }

    candidates
}

fn push_existing(candidates: &mut Vec<PathBuf>, path: PathBuf) {
    if path.is_file() && !candidates.iter().any(|candidate| candidate == &path) {
        candidates.push(path);
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(value)
}

fn output_text(output: &Output) -> String {
    format!(
        "{} {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    )
    .to_ascii_lowercase()
}

fn is_missing_item(output: &Output) -> bool {
    output.status.code() == Some(44)
        || [
            "could not be found in the keychain",
            "item not found",
            "errsecitemnotfound",
        ]
        .iter()
        .any(|needle| output_text(output).contains(needle))
}

fn is_missing_keychain(output: &Output) -> bool {
    let text = output_text(output);
    text.contains("specified keychain could not be found")
        || text.contains("no such keychain")
        || text.contains("unable to open keychain")
}

fn keychain_error(action: &str, output: &Output) -> anyhow::Error {
    let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let normalized = detail.to_ascii_lowercase();
    if is_missing_keychain(output) {
        anyhow::anyhow!(
            "could not {action} Joocode's local credential because no usable macOS login keychain was found; open Keychain Access once or set JOOCODE_KEYCHAIN to an existing user keychain"
        )
    } else if normalized.contains("user interaction is not allowed")
        || normalized.contains("authorization denied")
        || normalized.contains("keychain is locked")
    {
        anyhow::anyhow!(
            "could not {action} Joocode's local credential because the macOS keychain is locked; unlock it in Keychain Access and retry"
        )
    } else if detail.is_empty() {
        anyhow::anyhow!("could not {action} Joocode's local credential in the macOS keychain")
    } else {
        anyhow::anyhow!("could not {action} Joocode's local credential: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    fn output(code: i32, stderr: &str) -> Output {
        Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[test]
    fn recognizes_normal_missing_item() {
        assert!(is_missing_item(&output(
            44,
            "security: SecKeychainSearchCopyNext: The specified item could not be found in the keychain."
        )));
    }

    #[test]
    fn distinguishes_missing_keychain() {
        assert!(is_missing_keychain(&output(
            1,
            "security: SecKeychainItemCreateFromContent: The specified keychain could not be found."
        )));
    }

    #[test]
    fn removes_security_quotes() {
        assert_eq!(
            unquote("\"/tmp/login.keychain-db\""),
            "/tmp/login.keychain-db"
        );
    }

    #[test]
    fn push_existing_deduplicates_paths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.keychain-db");
        std::fs::write(&path, "test").unwrap();
        let mut candidates = Vec::new();
        push_existing(&mut candidates, path.clone());
        push_existing(&mut candidates, path);
        assert_eq!(candidates.len(), 1);
    }
}
