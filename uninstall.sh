#!/bin/sh
set -eu

INSTALL_DIR="${JOOCODE_INSTALL_DIR:-${JOC_INSTALL_DIR:-$HOME/.local/bin}}"
BINARY="$INSTALL_DIR/joocode"

usage() {
  cat <<'EOF'
Usage: uninstall.sh [--yes]

Removes the standalone Joocode binary from ~/.local/bin by default.

Environment:
  JOOCODE_INSTALL_DIR  Directory containing the joocode binary.

This script does not remove OpenCode credentials or configuration, nor Joocode
entries in Codex or Zed settings. Remove a Homebrew installation with:
  brew uninstall joocode
EOF
}

case "${1:-}" in
  "") confirm=true ;;
  --yes|-y) confirm=false ;;
  --help|-h) usage; exit 0 ;;
  *)
    usage >&2
    exit 2
    ;;
esac

if [ ! -e "$BINARY" ]; then
  printf 'joocode uninstaller: no binary found at %s\n' "$BINARY" >&2
  printf 'If installed with Homebrew, run: brew uninstall joocode\n' >&2
  exit 0
fi

if [ ! -f "$BINARY" ]; then
  printf 'joocode uninstaller: refusing to remove non-file path: %s\n' "$BINARY" >&2
  exit 1
fi

if [ "$confirm" = true ]; then
  printf 'Remove Joocode binary at %s? [y/N] ' "$BINARY"
  read -r answer || answer=""
  case "$answer" in
    y|Y|yes|YES) ;;
    *)
      printf 'Cancelled.\n'
      exit 0
      ;;
  esac
fi

rm -f "$BINARY"
printf 'Removed %s\n' "$BINARY"
printf 'OpenCode, Codex, and Zed configuration was preserved.\n'
