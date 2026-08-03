#!/usr/bin/env bash
set -euo pipefail

# Creates a release commit and matching annotated Git tag.
#
# Usage:
#   make release                  # next patch version
#   make release BUMP=minor       # next minor version
#   make release BUMP=major       # next major version
#   make release VERSION=1.2.3    # explicit version
#   make release DRY_RUN=1        # validate and show planned actions only

BUMP="${BUMP:-patch}"
VERSION="${VERSION:-}"
DRY_RUN="${DRY_RUN:-0}"

fail() {
  echo "error: $*" >&2
  exit 1
}

run() {
  if [[ "$DRY_RUN" == "1" ]]; then
    printf '+ '
    printf '%q ' "$@"
    printf '\n'
  else
    "$@"
  fi
}

[[ -f Cargo.toml ]] || fail "run this command from the repository root"
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || fail "not a Git repository"

branch="$(git branch --show-current)"
[[ "$branch" == "main" ]] || fail "releases must be created from main (current: ${branch:-detached HEAD})"

if [[ "$DRY_RUN" != "1" ]]; then
  git diff --quiet && git diff --cached --quiet || fail "working tree has uncommitted changes"
  git fetch origin main --tags
  [[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || fail "local main is not synchronized with origin/main"
fi

current_version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
[[ "$current_version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]] || fail "Cargo.toml version is not a stable semantic version: $current_version"

if [[ -n "$VERSION" ]]; then
  [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "VERSION must be x.y.z (received: $VERSION)"
  next_version="$VERSION"
else
  IFS='.' read -r major minor patch <<< "$current_version"
  case "$BUMP" in
    patch) next_version="$major.$minor.$((patch + 1))" ;;
    minor) next_version="$major.$((minor + 1)).0" ;;
    major) next_version="$((major + 1)).0.0" ;;
    *) fail "BUMP must be patch, minor, or major (received: $BUMP)" ;;
  esac
fi

[[ "$next_version" != "$current_version" ]] || fail "next version matches current version ($current_version)"
tag="v$next_version"
if [[ "$DRY_RUN" != "1" ]]; then
  git rev-parse -q --verify "refs/tags/$tag" >/dev/null && fail "tag already exists: $tag"
fi

echo "Preparing JustOpenCode $tag (from $current_version)."

if [[ "$DRY_RUN" == "1" ]]; then
  echo "Dry run: would update Cargo.toml and Cargo.lock, run the locked release gate, commit, tag, and push main plus $tag."
  exit 0
fi

python3 - "$next_version" <<'PY'
from pathlib import Path
import re
import sys

version = sys.argv[1]
path = Path("Cargo.toml")
content = path.read_text()
updated, count = re.subn(
    r'(?m)^(version\s*=\s*")[^"]+("\s*)$',
    rf'\g<1>{version}\g<2>',
    content,
    count=1,
)
if count != 1:
    raise SystemExit("could not update package version in Cargo.toml")
path.write_text(updated)
PY

cargo generate-lockfile
cargo fmt --all --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-features
cargo build --release --locked

git add Cargo.toml Cargo.lock
git diff --cached --quiet && fail "version update produced no changes"
git commit -m "chore: release $tag"
git tag -a "$tag" -m "JustOpenCode $tag"
git push origin main
git push origin "$tag"

echo "Released JustOpenCode $tag."
