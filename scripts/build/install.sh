#!/usr/bin/env bash
set -euo pipefail

# Build the ur CLI and builderd binaries and install them.
# ur-server is started via `ur start`.
# Set UR_BUILD_PROFILE=debug for debug builds (default: release).
# Set UR_INSTALL_DIR to override the install directory (default: ~/.local/bin).

PROFILE="${UR_BUILD_PROFILE:-release}"
INSTALL_DIR="${UR_INSTALL_DIR:-$HOME/.local/bin}"

if [ "$PROFILE" = "debug" ]; then
    cargo build -p ur -p urui -p builderd
    TARGET_DIR="target/debug"
else
    cargo build --release -p ur -p urui -p builderd
    TARGET_DIR="target/release"
fi

# Kill existing daemon before replacing binary
if [ -x "$INSTALL_DIR/ur" ]; then
    "$INSTALL_DIR/ur" kill server 2>/dev/null || true
fi

mkdir -p "$INSTALL_DIR" "$HOME/.ur/logs"

# Remove before copying: macOS caches code signature page hashes for running
# binaries. Overwriting in-place (cp) invalidates the cache, causing the kernel
# to SIGKILL the new binary on exec. Removing first creates a new inode.
rm -f "$INSTALL_DIR/ur" "$INSTALL_DIR/urui" "$INSTALL_DIR/builderd"
cp "$TARGET_DIR/ur" "$INSTALL_DIR/ur"
cp "$TARGET_DIR/urui" "$INSTALL_DIR/urui"
cp "$TARGET_DIR/builderd" "$INSTALL_DIR/builderd"
echo "Installed ur, urui, builderd to $INSTALL_DIR/"
echo "Run 'ur server start' to launch the server"
