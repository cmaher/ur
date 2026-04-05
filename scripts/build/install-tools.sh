#!/usr/bin/env bash
set -euo pipefail

# Build and install tool binaries (tk-import, tk-verify).
# These are independent of the core ur/builderd install.
# Set UR_BUILD_PROFILE=debug for debug builds (default: release).
# Set UR_INSTALL_DIR to override the install directory (default: ~/.local/bin).

PROFILE="${UR_BUILD_PROFILE:-release}"
INSTALL_DIR="${UR_INSTALL_DIR:-$HOME/.local/bin}"

if [ "$PROFILE" = "debug" ]; then
    cargo build -p tk-import -p tk-verify
    TARGET_DIR="target/debug"
else
    cargo build --release -p tk-import -p tk-verify
    TARGET_DIR="target/release"
fi

mkdir -p "$INSTALL_DIR"

rm -f "$INSTALL_DIR/tk-import" "$INSTALL_DIR/tk-verify"
cp "$TARGET_DIR/tk-import" "$INSTALL_DIR/tk-import"
cp "$TARGET_DIR/tk-verify" "$INSTALL_DIR/tk-verify"
echo "Installed tk-import, tk-verify to $INSTALL_DIR/"
