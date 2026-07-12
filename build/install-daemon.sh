#!/usr/bin/env bash
# Installs (or with --uninstall removes) temporald as a launchd LaunchAgent.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"

LABEL="com.temporal.temporald"
APP_DIR="$HOME/Library/Application Support/temporald"
BIN="$APP_DIR/bin/temporald"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
DOMAIN="gui/$(id -u)"

if [ "${1:-}" = "--uninstall" ]; then
    launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
    rm -f "$PLIST" "$BIN"
    echo "uninstalled $LABEL (data in '$APP_DIR' kept)"
    exit 0
fi

echo "==> building release binary"
(cd "$REPO_ROOT/daemon" && cargo build --release -p temporald)

echo "==> installing to $APP_DIR"
mkdir -p "$APP_DIR/bin" "$APP_DIR/logs" "$HOME/Library/LaunchAgents"
# Unload first so the running binary can be replaced.
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
cp "$REPO_ROOT/daemon/target/release/temporald" "$BIN"

sed -e "s|@BINARY@|$BIN|g" -e "s|@LOGDIR@|$APP_DIR/logs|g" \
    "$REPO_ROOT/build/launchd/$LABEL.plist.template" > "$PLIST"

echo "==> loading agent"
launchctl bootstrap "$DOMAIN" "$PLIST"
launchctl kickstart -k "$DOMAIN/$LABEL"

sleep 1
"$BIN" probe ping && echo "temporald is up: $LABEL loaded in $DOMAIN"
