#!/usr/bin/env bash
set -euo pipefail

# Build the ur CLI binary and install to ~/bin.
# ur-server runs in a container (built by container-image task), not installed locally.
# Set UR_BUILD_PROFILE=debug for debug builds (default: release).

PROFILE="${UR_BUILD_PROFILE:-release}"

if [ "$PROFILE" = "debug" ]; then
    cargo build -p ur
    TARGET_DIR="target/debug"
else
    cargo build --release -p ur
    TARGET_DIR="target/release"
fi

# Kill existing daemon before replacing binary
if [ -x "$HOME/bin/ur" ]; then
    "$HOME/bin/ur" kill server 2>/dev/null || true
fi

mkdir -p "$HOME/bin" "$HOME/.ur/logs"
cp "$TARGET_DIR/ur" "$HOME/bin/ur"
echo "Installed ur to $HOME/bin/"
echo "ur-server runs in a container — use 'docker compose -f containers/docker-compose.yml up -d' to start"
