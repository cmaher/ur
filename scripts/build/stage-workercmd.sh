#!/usr/bin/env bash
set -euo pipefail

# Cross-compile workercmd binaries for linux-gnu and stage for Dockerfile
# Requires: zig + cargo-zigbuild

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    x86_64)        TARGET="x86_64-unknown-linux-gnu" ;;
    *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

echo "Cross-compiling workercmd binaries for $TARGET"
cargo zigbuild --release --target "$TARGET" -p ur-ping -p workercmd-git -p workercmd-gh

DEST=containers/claude-worker
cp "target/$TARGET/release/ur-ping" "$DEST/ur-ping"
cp "target/$TARGET/release/git" "$DEST/git"
cp "target/$TARGET/release/gh" "$DEST/gh"

rm -f "$DEST/tk"
TK_PATH=$(which tk 2>/dev/null || true)
if [ -n "$TK_PATH" ]; then
    cp "$TK_PATH" "$DEST/tk"
    echo "Staged tk from $TK_PATH"
else
    printf '#!/bin/sh\necho "tk stub: $*"\n' > "$DEST/tk"
    echo "Staged tk stub (real tk not found)"
fi

echo "Staged workercmd binaries in $DEST/"
