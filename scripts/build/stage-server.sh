#!/usr/bin/env bash
set -euo pipefail

# Raise fd limit — the zig linker opens hundreds of .rlib files simultaneously
# and can exceed macOS's default soft limit (256).
ulimit -n 65536 2>/dev/null || true

# Cross-compile ur-server binary for linux-gnu (Debian) and stage for Dockerfile.
# Requires: zig + cargo-zigbuild

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    x86_64)        TARGET="x86_64-unknown-linux-gnu" ;;
    *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

echo "Cross-compiling ur-server for $TARGET"
cargo zigbuild --release --target "$TARGET" -p ur-server

DEST=containers/server
cp "target/$TARGET/release/ur-server" "$DEST/ur-server"

echo "Staged ur-server binary in $DEST/"
