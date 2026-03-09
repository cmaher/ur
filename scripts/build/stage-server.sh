#!/usr/bin/env bash
set -euo pipefail

# Cross-compile urd binary for linux-musl (Alpine) and stage for Dockerfile.
# Requires: zig + cargo-zigbuild

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-unknown-linux-musl" ;;
    x86_64)        TARGET="x86_64-unknown-linux-musl" ;;
    *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

echo "Cross-compiling urd for $TARGET"
cargo zigbuild --release --target "$TARGET" -p urd

DEST=containers/server
cp "target/$TARGET/release/urd" "$DEST/urd"

echo "Staged urd binary in $DEST/"
