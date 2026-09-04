use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand, ValueEnum};
use semver::Version;
use toml_edit::DocumentMut;

const DEFAULT_REPOSITORY: &str = "yan-ad/joocode";
const FORMULA_ASSETS: [(&str, &str); 4] = [
    ("arm64_macos", "joocode-aarch64-apple-darwin.tar.gz"),
    ("x86_macos", "joocode-x86_64-apple-darwin.tar.gz"),
    ("arm64_linux", "joocode-aarch64-unknown-linux-gnu.tar.gz"),
    ("x86_linux", "joocode-x86_64-unknown-linux-gnu.tar.gz"),
];

#[derive(Debug, Parser)]
#[command(
    name = "cargo xtask",
    about = "Native Joocode development and release tooling"
)]
struct Cli {
    #[command(subcommand)]
    command: XtaskCommand,
}

#[derive(Debug, Subcommand)]
enum XtaskCommand {
    /// Run the complete locked CI/release validation gate.
    Check,
    /// Generate a checksum-pinned Homebrew formula.
    HomebrewFormula {
        /// Release tag or version, for example v1.2.3.
        version: String,
        /// Path to SHA256SUMS.
        checksums: PathBuf,
        /// Formula output path.
        #[arg(short, long, default_value = "joocode.rb")]
        output: PathBuf,
        /// GitHub owner/repository override.
        #[arg(long, env = "GITHUB_REPOSITORY")]
        repository: Option<String>,
    },
    /// Generate deterministic release notes from conventional commits.
    ReleaseNotes {
        /// Existing release tag.
        tag: String,
        /// Markdown output path.
        output: PathBuf,
        /// GitHub owner/repository override.
        #[arg(long, env = "GITHUB_REPOSITORY")]
        repository: Option<String>,
    },
    /// Verify that a tag matches the root Cargo package version.
    VerifyTag { tag: String },
    /// Bump, validate, commit, tag, and push a release.
    Release {
        /// Semantic version component to increment.
        #[arg(long, value_enum, default_value_t = Bump::Patch)]
        bump: Bump,
        /// Explicit x.y.z version instead of an automatic bump.
        #[arg(long)]
        version: Option<Version>,
        /// Print the planned release without changing files or Git history.
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum Bump {
    #[default]
    Patch,
    Minor,
    Major,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = repository_root()?;
    match cli.command {
        XtaskCommand::Check => run_checks(&root),
        XtaskCommand::HomebrewFormula {
            version,
            checksums,
            output,
            repository,
        } => {
            let checksums = absolute_from(&root, &checksums);
            let output = absolute_from(&root, &output);
            generate_homebrew_formula(
                &version,
                &checksums,
                &output,
                repository.as_deref().unwrap_or(DEFAULT_REPOSITORY),
            )?;
            println!("generated {}", output.display());
            Ok(())
        }
        XtaskCommand::ReleaseNotes {
            tag,
            output,
            repository,
        } => {
            let output = absolute_from(&root, &output);
            generate_release_notes(
                &root,
                &tag,
                &output,
                repository.as_deref().unwrap_or(DEFAULT_REPOSITORY),
            )?;
            println!("generated {}", output.display());
            Ok(())
        }
        XtaskCommand::VerifyTag { tag } => verify_tag(&root, &tag),
        XtaskCommand::Release {
            bump,
            version,
            dry_run,
        } => release(&root, bump, version, dry_run),
    }
}

fn repository_root() -> Result<PathBuf> {
    let output = command_output(Path::new("."), "git", &["rev-parse", "--show-toplevel"])
        .context("run this command from the Joocode Git repository")?;
    Ok(PathBuf::from(output.trim()))
}

fn absolute_from(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn run_checks(root: &Path) -> Result<()> {
    run(root, "cargo", &["fmt", "--all", "--check"])?;
    run(
        root,
        "cargo",
        &[
            "clippy",
            "--locked",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ],
    )?;
    run(
        root,
        "cargo",
        &["test", "--locked", "--workspace", "--all-features"],
    )?;
    run(
        root,
        "cargo",
        &["build", "--release", "--locked", "-p", "joocode"],
    )
}

fn parse_checksums(path: &Path) -> Result<BTreeMap<String, String>> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read checksums from {}", path.display()))?;
    let mut checksums = BTreeMap::new();
    for (index, line) in contents.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split_whitespace();
        let digest = fields
            .next()
            .with_context(|| format!("invalid checksum line {}", index + 1))?;
        let filename = fields
            .next()
            .with_context(|| format!("missing filename on checksum line {}", index + 1))?
            .trim_start_matches('*');
        ensure!(
            digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "invalid SHA-256 digest on line {}",
            index + 1
        );
        checksums.insert(filename.to_owned(), digest.to_owned());
    }
    Ok(checksums)
}

fn generate_homebrew_formula(
    version: &str,
    checksums_path: &Path,
    output: &Path,
    repository: &str,
) -> Result<()> {
    let version = version.strip_prefix('v').unwrap_or(version);
    Version::parse(version).context("Homebrew version must be semantic version x.y.z")?;
    let checksums = parse_checksums(checksums_path)?;
    for (_, asset) in FORMULA_ASSETS {
        ensure!(checksums.contains_key(asset), "missing checksum: {asset}");
    }
    let asset = |key: &str| {
        FORMULA_ASSETS
            .iter()
            .find_map(|(candidate, asset)| (*candidate == key).then_some(*asset))
            .expect("known formula asset")
    };
    let checksum = |key: &str| &checksums[asset(key)];
    let url = |key: &str| {
        format!(
            "https://github.com/{repository}/releases/download/v{version}/{}",
            asset(key)
        )
    };
    let formula = format!(
        r##"class Joocode < Formula
  desc "Native bridge from configured AI providers to local AI clients"
  homepage "https://github.com/{repository}"
  version "{version}"
  license "MIT"

  on_macos do
    if Hardware::CPU.arm?
      url "{}"
      sha256 "{}"
    else
      url "{}"
      sha256 "{}"
    end
  end

  on_linux do
    if Hardware::CPU.arm?
      url "{}"
      sha256 "{}"
    else
      url "{}"
      sha256 "{}"
    end
  end

  def install
    bin.install Dir["joocode-*/jcx"].first => "jcx"
    bin.install Dir["joocode-*/joocode"].first => "joocode"
  end

  test do
    assert_match "jcx", shell_output("#{{bin}}/jcx --help")
  end
end
"##,
        url("arm64_macos"),
        checksum("arm64_macos"),
        url("x86_macos"),
        checksum("x86_macos"),
        url("arm64_linux"),
        checksum("arm64_linux"),
        url("x86_linux"),
        checksum("x86_linux"),
    );
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, formula)
        .with_context(|| format!("failed to write formula to {}", output.display()))
}

fn previous_tag(root: &Path, tag: &str) -> Result<Option<String>> {
    let tags = command_output(
        root,
        "git",
        &[
            "tag",
            "--merged",
            &format!("{tag}^{{}}"),
            "--sort=-version:refname",
        ],
    )?;
    Ok(tags
        .lines()
        .find(|candidate| *candidate != tag)
        .map(str::to_owned))
}

fn generate_release_notes(root: &Path, tag: &str, output: &Path, repository: &str) -> Result<()> {
    run(
        root,
        "git",
        &["rev-parse", "--verify", &format!("refs/tags/{tag}")],
    )?;
    let previous = previous_tag(root, tag)?;
    let revision_range = previous
        .as_ref()
        .map_or_else(|| tag.to_owned(), |previous| format!("{previous}..{tag}"));
    let log = command_output(root, "git", &["log", "--format=%H%x09%s", &revision_range])?;
    let headings = [
        "Features",
        "Fixes",
        "Performance",
        "Documentation",
        "Build and maintenance",
        "Other changes",
    ];
    let mut categories = BTreeMap::<&str, Vec<String>>::new();
    for heading in headings {
        categories.insert(heading, Vec::new());
    }
    for line in log.lines() {
        let Some((commit, subject)) = line.split_once('\t') else {
            continue;
        };
        if is_release_commit(subject) {
            continue;
        }
        let (heading, title) = classify_commit(subject);
        let short = commit.get(..7).unwrap_or(commit);
        categories.entry(heading).or_default().push(format!(
            "- {title} ([`{short}`](https://github.com/{repository}/commit/{commit}))"
        ));
    }
    let mut lines = vec![
        format!("# Joocode {tag}"),
        String::new(),
        "## What's changed".to_owned(),
        String::new(),
    ];
    if categories.values().all(Vec::is_empty) {
        lines.push("- No user-facing changes were recorded.".to_owned());
        lines.push(String::new());
    } else {
        for heading in headings {
            let entries = &categories[heading];
            if entries.is_empty() {
                continue;
            }
            lines.push(format!("### {heading}"));
            lines.push(String::new());
            lines.extend(entries.iter().cloned());
            lines.push(String::new());
        }
    }
    if let Some(previous) = previous {
        lines.extend([
            "## Full changelog".to_owned(),
            String::new(),
            format!(
                "[{previous}...{tag}](https://github.com/{repository}/compare/{previous}...{tag})"
            ),
            String::new(),
        ]);
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, lines.join("\n"))
        .with_context(|| format!("failed to write release notes to {}", output.display()))
}

fn is_release_commit(subject: &str) -> bool {
    let lower = subject.to_ascii_lowercase();
    let Some((prefix, title)) = lower.split_once(':') else {
        return false;
    };
    (prefix == "chore" || (prefix.starts_with("chore(") && prefix.ends_with(')')))
        && title.trim_start().starts_with("release v")
}

fn classify_commit(subject: &str) -> (&'static str, String) {
    let Some((prefix, title)) = subject.split_once(':') else {
        return ("Other changes", subject.to_owned());
    };
    let breaking = prefix.ends_with('!');
    let kind = prefix
        .trim_end_matches('!')
        .split_once('(')
        .map_or(prefix.trim_end_matches('!'), |(kind, _)| kind);
    let heading = match kind {
        "feat" => "Features",
        "fix" => "Fixes",
        "perf" => "Performance",
        "docs" => "Documentation",
        "build" | "ci" | "chore" | "refactor" | "test" => "Build and maintenance",
        _ => "Other changes",
    };
    let title = title.trim();
    let title = if breaking {
        format!("**Breaking:** {title}")
    } else {
        title.to_owned()
    };
    (heading, title)
}

fn root_package_version(root: &Path) -> Result<Version> {
    let manifest_path = root.join("Cargo.toml");
    let document = fs::read_to_string(&manifest_path)?
        .parse::<DocumentMut>()
        .context("failed to parse Cargo.toml")?;
    let version = document["package"]["version"]
        .as_str()
        .context("Cargo.toml is missing package.version")?;
    Version::parse(version).context("Cargo.toml package version is not valid semantic version")
}

fn verify_tag(root: &Path, tag: &str) -> Result<()> {
    let version = root_package_version(root)?;
    ensure!(
        tag == format!("v{version}"),
        "tag {tag} does not match Cargo version v{version}"
    );
    Ok(())
}

fn next_version(current: &Version, bump: Bump) -> Version {
    match bump {
        Bump::Patch => Version::new(current.major, current.minor, current.patch + 1),
        Bump::Minor => Version::new(current.major, current.minor + 1, 0),
        Bump::Major => Version::new(current.major + 1, 0, 0),
    }
}

fn release(root: &Path, bump: Bump, explicit: Option<Version>, dry_run: bool) -> Result<()> {
    let branch = command_output(root, "git", &["branch", "--show-current"])?;
    ensure!(
        branch == "main",
        "releases must be created from main (current: {branch})"
    );
    let current = root_package_version(root)?;
    let next = explicit.unwrap_or_else(|| next_version(&current, bump));
    ensure!(
        next != current,
        "next version matches current version ({current})"
    );
    let tag = format!("v{next}");
    println!("Preparing Joocode {tag} (from {current}).");
    if dry_run {
        println!(
            "Dry run: would update Cargo.toml and Cargo.lock, run the locked release gate, commit, tag, and push main plus {tag}."
        );
        return Ok(());
    }
    ensure_clean(root)?;
    run(root, "git", &["fetch", "origin", "main", "--tags"])?;
    let head = command_output(root, "git", &["rev-parse", "HEAD"])?;
    let origin = command_output(root, "git", &["rev-parse", "origin/main"])?;
    ensure!(
        head == origin,
        "local main is not synchronized with origin/main"
    );
    ensure!(
        !git_ref_exists(root, &format!("refs/tags/{tag}"))?,
        "tag already exists: {tag}"
    );
    update_manifest_version(root, &next)?;
    run(root, "cargo", &["generate-lockfile"])?;
    run_checks(root)?;
    run(root, "git", &["add", "Cargo.toml", "Cargo.lock"])?;
    let staged = command_status(root, "git", &["diff", "--cached", "--quiet"])?;
    ensure!(!staged.success(), "version update produced no changes");
    run(
        root,
        "git",
        &["commit", "-m", &format!("chore: release {tag}")],
    )?;
    run(
        root,
        "git",
        &["tag", "-a", &tag, "-m", &format!("Joocode {tag}")],
    )?;
    run(root, "git", &["push", "origin", "main"])?;
    run(root, "git", &["push", "origin", &tag])?;
    println!("Released Joocode {tag}.");
    Ok(())
}

fn update_manifest_version(root: &Path, version: &Version) -> Result<()> {
    let path = root.join("Cargo.toml");
    let mut document = fs::read_to_string(&path)?
        .parse::<DocumentMut>()
        .context("failed to parse Cargo.toml")?;
    document["package"]["version"] = toml_edit::value(version.to_string());
    fs::write(&path, document.to_string())?;
    Ok(())
}

fn ensure_clean(root: &Path) -> Result<()> {
    let status = command_output(root, "git", &["status", "--porcelain"])?;
    ensure!(status.is_empty(), "working tree has uncommitted changes");
    Ok(())
}

fn git_ref_exists(root: &Path, reference: &str) -> Result<bool> {
    Ok(command_status(
        root,
        "git",
        &["rev-parse", "--quiet", "--verify", reference],
    )?
    .success())
}

fn run(root: &Path, program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("failed to run {program}"))?;
    ensure!(status.success(), "{program} {} failed", args.join(" "));
    Ok(())
}

fn command_status(root: &Path, program: &str, args: &[&str]) -> Result<std::process::ExitStatus> {
    Command::new(program)
        .args(args)
        .current_dir(root)
        .status()
        .with_context(|| format!("failed to run {program}"))
}

fn command_output(root: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    output_text(program, args, output)
}

fn output_text(program: &str, args: &[&str], output: Output) -> Result<String> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        bail!("{program} {} failed: {stderr}", args.join(" "));
    }
    Ok(String::from_utf8(output.stdout)
        .context("command output was not UTF-8")?
        .trim()
        .to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn classifies_conventional_commits() {
        assert_eq!(classify_commit("feat(ui): add dashboard").0, "Features");
        assert_eq!(
            classify_commit("fix!: remove legacy path").1,
            "**Breaking:** remove legacy path"
        );
        assert_eq!(classify_commit("plain subject").0, "Other changes");
    }

    #[test]
    fn bumps_semantic_versions() {
        let version = Version::new(1, 2, 3);
        assert_eq!(next_version(&version, Bump::Patch), Version::new(1, 2, 4));
        assert_eq!(next_version(&version, Bump::Minor), Version::new(1, 3, 0));
        assert_eq!(next_version(&version, Bump::Major), Version::new(2, 0, 0));
    }

    #[test]
    fn generates_formula_with_both_commands() {
        let temp = tempdir().unwrap();
        let sums = temp.path().join("SHA256SUMS");
        let contents = FORMULA_ASSETS
            .iter()
            .enumerate()
            .map(|(index, (_, asset))| format!("{:064x}  {asset}", index + 1))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(&sums, contents).unwrap();
        let output = temp.path().join("joocode.rb");
        generate_homebrew_formula("v1.2.3", &sums, &output, "owner/repo").unwrap();
        let formula = fs::read_to_string(output).unwrap();
        assert!(formula.contains("class Joocode < Formula"));
        assert!(formula.contains("releases/download/v1.2.3"));
        assert!(formula.contains("=> \"jcx\""));
        assert!(formula.contains("=> \"joocode\""));
    }

    #[test]
    fn updates_only_root_package_version() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\n\n[dependencies]\nfoo = \"1\"\n",
        )
        .unwrap();
        update_manifest_version(temp.path(), &Version::new(2, 0, 0)).unwrap();
        let manifest = fs::read_to_string(temp.path().join("Cargo.toml")).unwrap();
        assert!(manifest.contains("version = \"2.0.0\""));
        assert!(manifest.contains("foo = \"1\""));
    }

    #[test]
    fn generates_grouped_release_notes_from_git_history() {
        let temp = tempdir().unwrap();
        let root = temp.path();
        run(root, "git", &["init", "-q"]).unwrap();
        run(root, "git", &["config", "user.name", "Test User"]).unwrap();
        run(root, "git", &["config", "user.email", "test@example.com"]).unwrap();

        fs::write(root.join("file.txt"), "one\n").unwrap();
        run(root, "git", &["add", "file.txt"]).unwrap();
        run(root, "git", &["commit", "-qm", "feat: add native task"]).unwrap();
        run(root, "git", &["tag", "v1.0.0"]).unwrap();

        fs::write(root.join("file.txt"), "two\n").unwrap();
        run(root, "git", &["add", "file.txt"]).unwrap();
        run(root, "git", &["commit", "-qm", "fix(ci): repair workflow"]).unwrap();
        run(root, "git", &["tag", "v1.0.1"]).unwrap();

        let output = root.join("RELEASE_NOTES.md");
        generate_release_notes(root, "v1.0.1", &output, "owner/repo").unwrap();
        let notes = fs::read_to_string(output).unwrap();
        assert!(notes.contains("# Joocode v1.0.1"));
        assert!(notes.contains("### Fixes"));
        assert!(notes.contains("repair workflow"));
        assert!(notes.contains("v1.0.0...v1.0.1"));
        assert!(!notes.contains("add native task"));
    }
}
