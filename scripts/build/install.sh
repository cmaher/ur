#!/usr/bin/env bash
set -euo pipefail

# Build binaries, install to ~/bin.
# Set UR_BUILD_PROFILE=debug for debug builds (default: release).

PROFILE="${UR_BUILD_PROFILE:-release}"

if [ "$PROFILE" = "debug" ]; then
    cargo build -p ur -p urd
    TARGET_DIR="target/debug"
else
    cargo build --release -p ur -p urd
    TARGET_DIR="target/release"
fi

# Kill existing daemon before replacing binary
if [ -x "$HOME/bin/ur" ]; then
    "$HOME/bin/ur" kill server 2>/dev/null || true
fi

mkdir -p "$HOME/bin" "$HOME/.ur/logs"
cp "$TARGET_DIR/ur" "$HOME/bin/ur"
cp "$TARGET_DIR/urd" "$HOME/bin/urd"
echo "Installed ur and urd to $HOME/bin/"
