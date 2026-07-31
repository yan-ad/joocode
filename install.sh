#!/bin/sh
set -eu

REPOSITORY="${CRABCODEX_REPOSITORY:-yan-ad/crabcodex}"
INSTALL_DIR="${CRABCODEX_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${CRABCODEX_VERSION:-${1:-latest}}"

fail() {
  printf 'crabcodex installer: %s\n' "$1" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v tar >/dev/null 2>&1 || fail "tar is required"

case "$(uname -s)" in
  Linux) os="unknown-linux-gnu" ;;
  Darwin) os="apple-darwin" ;;
  *) fail "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac

target="${arch}-${os}"
asset="crabcodex-${target}.tar.gz"

if [ "$VERSION" = "latest" ]; then
  latest_url="$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/${REPOSITORY}/releases/latest")" \
    || fail "could not resolve the latest release"
  VERSION="${latest_url##*/}"
fi

case "$VERSION" in
  v*) ;;
  *) VERSION="v${VERSION}" ;;
esac

base_url="https://github.com/${REPOSITORY}/releases/download/${VERSION}"
tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t crabcodex)"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

printf 'Downloading CrabCodex %s for %s...\n' "$VERSION" "$target"
curl -fL --retry 3 --proto '=https' --tlsv1.2 \
  "$base_url/$asset" -o "$tmp_dir/$asset" \
  || fail "release asset not found: $asset"
curl -fL --retry 3 --proto '=https' --tlsv1.2 \
  "$base_url/SHA256SUMS" -o "$tmp_dir/SHA256SUMS" \
  || fail "could not download SHA256SUMS"

expected="$(awk -v asset="$asset" '$2 == asset || $2 == "*" asset { print $1; exit }' "$tmp_dir/SHA256SUMS")"
[ -n "$expected" ] || fail "checksum for $asset is missing"

if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$tmp_dir/$asset" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$tmp_dir/$asset" | awk '{print $1}')"
else
  fail "sha256sum or shasum is required to verify the download"
fi

[ "$actual" = "$expected" ] || fail "checksum verification failed"

tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"
mkdir -p "$INSTALL_DIR"
install -m 755 "$tmp_dir/crabcodex-${target}/crabcodex" "$INSTALL_DIR/crabcodex"

printf '\nCrabCodex %s installed to %s/crabcodex\n' "$VERSION" "$INSTALL_DIR"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf 'Add %s to PATH to run crabcodex globally.\n' "$INSTALL_DIR" ;;
esac
printf '\nNext steps:\n'
printf '  crabcodex doctor\n'
printf '  crabcodex codex-install\n'
printf '  crabcodex serve\n'
