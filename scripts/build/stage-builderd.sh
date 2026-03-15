#!/usr/bin/env bash
set -euo pipefail

# Raise fd limit — the zig linker opens hundreds of .rlib files simultaneously
# and can exceed macOS's default soft limit (256).
ulimit -n 65536 2>/dev/null || true

# Cross-compile builderd binary for linux-gnu (Debian) and stage for container build context.
# Single target: aarch64 only (builderd runs on ARM64 Linux builders).
# Requires: zig + cargo-zigbuild

TARGET="aarch64-unknown-linux-gnu"

echo "Cross-compiling builderd for $TARGET"
cargo zigbuild --release --target "$TARGET" -p builderd

DEST=containers/server
cp "target/$TARGET/release/builderd" "$DEST/builderd"

echo "Staged builderd binary in $DEST/"
