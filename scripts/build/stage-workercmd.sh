#!/usr/bin/env bash
set -euo pipefail

# Cross-compile worker binaries for linux-gnu and stage for Dockerfile
# Requires: zig + cargo-zigbuild

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    x86_64)        TARGET="x86_64-unknown-linux-gnu" ;;
    *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

echo "Cross-compiling worker binaries for $TARGET"
cargo zigbuild --release --target "$TARGET" -p ur-ping -p workertools -p workerd

DEST=containers/claude-worker/bin
mkdir -p "$DEST"
cp "target/$TARGET/release/ur-ping" "$DEST/ur-ping"
cp "target/$TARGET/release/workertools" "$DEST/workertools"
cp "target/$TARGET/release/workerd" "$DEST/workerd"

rm -f "$DEST/tk"
TK_PATH=$(which tk 2>/dev/null || true)
if [ -n "$TK_PATH" ]; then
    cp "$TK_PATH" "$DEST/tk"
    echo "Staged tk from $TK_PATH"
else
    printf '#!/bin/sh\necho "tk stub: $*"\n' > "$DEST/tk"
    echo "Staged tk stub (real tk not found)"
fi

echo "Staged worker binaries in $DEST/"
