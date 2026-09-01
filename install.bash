#!/usr/bin/env sh
set -eu

REPOSITORY="${JOOCODE_REPOSITORY:-${JOC_REPOSITORY:-yan-ad/joocode}}"
INSTALL_DIR="${JOOCODE_INSTALL_DIR:-${JOC_INSTALL_DIR:-$HOME/.local/bin}}"
VERSION="${JOOCODE_VERSION:-${JOC_VERSION:-${1:-latest}}}"

fail() {
  printf 'joocode installer: %s\n' "$1" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail "curl is required"

case "$(uname -s)" in
  Linux) os="unknown-linux-gnu"; archive="tar.gz"; binary="jcx"; legacy_binary="joocode" ;;
  Darwin) os="apple-darwin"; archive="tar.gz"; binary="jcx"; legacy_binary="joocode" ;;
  MINGW*|MSYS*|CYGWIN*) os="pc-windows-msvc"; archive="zip"; binary="jcx.exe"; legacy_binary="joocode.exe" ;;
  *) fail "unsupported operating system: $(uname -s)" ;;
esac

case "$(uname -m)" in
  x86_64|amd64) arch="x86_64" ;;
  arm64|aarch64) arch="aarch64" ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac

target="${arch}-${os}"
asset="joocode-${target}.${archive}"

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
tmp_dir="$(mktemp -d 2>/dev/null || mktemp -d -t joocode)"
trap 'rm -rf "$tmp_dir"' EXIT INT TERM

printf 'Downloading Joocode %s for %s...\n' "$VERSION" "$target"
curl -fL --retry 3 --proto '=https' --tlsv1.2 "$base_url/$asset" -o "$tmp_dir/$asset" \
  || fail "release asset not found: $asset"
curl -fL --retry 3 --proto '=https' --tlsv1.2 "$base_url/SHA256SUMS" -o "$tmp_dir/SHA256SUMS" \
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

case "$archive" in
  tar.gz)
    command -v tar >/dev/null 2>&1 || fail "tar is required"
    tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"
    source="$tmp_dir/joocode-${target}/$binary"
    source_root="$tmp_dir/joocode-${target}"
    ;;
  zip)
    command -v unzip >/dev/null 2>&1 || fail "unzip is required (use install.ps1 from PowerShell if unavailable)"
    unzip -q "$tmp_dir/$asset" -d "$tmp_dir/extracted"
    source="$tmp_dir/extracted/joocode-${target}/$binary"
    source_root="$tmp_dir/extracted/joocode-${target}"
    ;;
esac

[ -f "$source" ] || fail "archive did not contain $binary"
mkdir -p "$INSTALL_DIR"
cp "$source" "$INSTALL_DIR/$binary"
[ "$archive" = "zip" ] || chmod 755 "$INSTALL_DIR/$binary"
legacy_source="$source_root/$legacy_binary"
if [ -f "$legacy_source" ]; then
  cp "$legacy_source" "$INSTALL_DIR/$legacy_binary"
  [ "$archive" = "zip" ] || chmod 755 "$INSTALL_DIR/$legacy_binary"
fi

case "$(uname -s)" in
  Darwin)
    if [ -d "$source_root/Joocode.app" ]; then
      app_dir="${JOOCODE_APP_DIR:-$HOME/Applications}/Joocode.app"
      mkdir -p "$(dirname "$app_dir")"
      rm -rf "$app_dir"
      cp -R "$source_root/Joocode.app" "$app_dir"
      printf 'Joocode app installed to %s\n' "$app_dir"
    fi
    ;;
  Linux)
    if [ -f "$source_root/joocode-icon.png" ]; then
      icon_dir="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/512x512/apps"
      desktop_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
      mkdir -p "$icon_dir" "$desktop_dir"
      cp "$source_root/joocode-icon.png" "$icon_dir/joocode.png"
      sed "s#^Exec=.*#Exec=$INSTALL_DIR/jcx#" "$source_root/joocode.desktop" > "$desktop_dir/joocode.desktop"
    fi
    ;;
esac

printf '\nJoocode %s installed to %s/%s\n' "$VERSION" "$INSTALL_DIR" "$binary"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf 'Add %s to PATH to run jcx globally.\n' "$INSTALL_DIR" ;;
esac
printf '\nNext steps:\n'
printf '  jcx doctor\n'
printf '  jcx codex-install\n'
printf '  jcx\n'
