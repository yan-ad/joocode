#!/bin/sh
set -eu

for candidate in \
  "$HOME/.local/bin/joocode" \
  "/opt/homebrew/bin/joocode" \
  "/usr/local/bin/joocode"
do
  if [ -x "$candidate" ]; then
    exec "$candidate"
  fi
done

/usr/bin/osascript -e 'display alert "Joocode is not installed" message "Install the Joocode CLI first, then reopen the app." as critical'
