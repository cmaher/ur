#!/usr/bin/env bash
set -euo pipefail

# Cross-compile ur-server binary for linux-musl (Alpine) and stage for Dockerfile.
# Requires: zig + cargo-zigbuild

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    x86_64)        TARGET="x86_64-unknown-linux-musl" ;;
    *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

echo "Cross-compiling ur-server for $TARGET"
cargo zigbuild --release --target "$TARGET" -p ur-server

DEST=containers/server
cp "target/$TARGET/release/ur-server" "$DEST/ur-server"

echo "Staged ur-server binary in $DEST/"
