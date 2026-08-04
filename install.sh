#!/usr/bin/env sh
# Backward-compatible installer entrypoint. Prefer install.bash for new users.
# This form also works when piped from curl, where no sibling file is present.
set -eu

if [ -f "$(dirname "$0")/install.bash" ]; then
  exec "$(dirname "$0")/install.bash" "$@"
fi

repository="${JOOCODE_REPOSITORY:-${JOC_REPOSITORY:-yan-ad/joocode}}"
command -v curl >/dev/null 2>&1 || {
  printf 'joocode installer: curl is required\n' >&2
  exit 1
}

curl --proto '=https' --tlsv1.2 -LsSf \
  "https://raw.githubusercontent.com/${repository}/main/install.bash" | sh -s -- "$@"
