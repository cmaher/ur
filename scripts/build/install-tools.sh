#!/usr/bin/env bash
set -euo pipefail

# Build and install all tool binaries found in tools/.
# Each subdirectory with a Cargo.toml is treated as a tool crate.
# Set UR_BUILD_PROFILE=debug for debug builds (default: release).
# Set UR_INSTALL_DIR to override the install directory (default: ~/.local/bin).

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

PROFILE="${UR_BUILD_PROFILE:-release}"
INSTALL_DIR="${UR_INSTALL_DIR:-$HOME/.local/bin}"

# Discover tool crates from tools/ directory
TOOLS=()
for dir in "$REPO_ROOT"/tools/*/; do
    [ -f "$dir/Cargo.toml" ] || continue
    TOOLS+=("$(basename "$dir")")
done

if [ ${#TOOLS[@]} -eq 0 ]; then
    echo "No tool crates found in tools/"
    exit 0
fi

# Build all tools
BUILD_ARGS=()
for tool in "${TOOLS[@]}"; do
    BUILD_ARGS+=("-p" "$tool")
done

if [ "$PROFILE" = "debug" ]; then
    cargo build "${BUILD_ARGS[@]}"
    TARGET_DIR="target/debug"
else
    cargo build --release "${BUILD_ARGS[@]}"
    TARGET_DIR="target/release"
fi

# Install all tools
mkdir -p "$INSTALL_DIR"
for tool in "${TOOLS[@]}"; do
    rm -f "$INSTALL_DIR/$tool"
    cp "$TARGET_DIR/$tool" "$INSTALL_DIR/$tool"
done

echo "Installed ${TOOLS[*]} to $INSTALL_DIR/"
