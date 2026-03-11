#!/usr/bin/env bash
set -euo pipefail

# Build the ur CLI and ur-hostd binaries and install them.
# ur-server runs in a container (built by image task), not installed locally.
# Set UR_BUILD_PROFILE=debug for debug builds (default: release).
# Set UR_INSTALL_DIR to override the install directory (default: ~/bin).

PROFILE="${UR_BUILD_PROFILE:-release}"
INSTALL_DIR="${UR_INSTALL_DIR:-$HOME/bin}"

if [ "$PROFILE" = "debug" ]; then
    cargo build -p ur -p ur-hostd
    TARGET_DIR="target/debug"
else
    cargo build --release -p ur -p ur-hostd
    TARGET_DIR="target/release"
fi

# Kill existing daemon before replacing binary
if [ -x "$INSTALL_DIR/ur" ]; then
    "$INSTALL_DIR/ur" kill server 2>/dev/null || true
fi

mkdir -p "$INSTALL_DIR" "$HOME/.ur/logs"
cp "$TARGET_DIR/ur" "$INSTALL_DIR/ur"
cp "$TARGET_DIR/ur-hostd" "$INSTALL_DIR/ur-hostd"
echo "Installed ur and ur-hostd to $INSTALL_DIR/"
echo "ur-server runs in a container — use 'docker compose -f containers/docker-compose.yml up -d' to start"
