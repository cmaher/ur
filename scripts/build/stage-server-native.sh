#!/usr/bin/env bash
set -euo pipefail

# Build ur-server binary for linux-gnu (Debian) and stage for Dockerfile (for Linux CI).
# Uses gnu target (not musl) because fastembed/ort requires dlopen for ONNX runtime.
# Uses cargo-zigbuild (available via mise).

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    x86_64)        TARGET="x86_64-unknown-linux-gnu" ;;
    *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

echo "Building ur-server for $TARGET"
rustup target add "$TARGET" 2>/dev/null || true
cargo zigbuild --release --target "$TARGET" -p ur-server

DEST=containers/server
cp "target/$TARGET/release/ur-server" "$DEST/ur-server"

echo "Staged ur-server binary in $DEST/"
