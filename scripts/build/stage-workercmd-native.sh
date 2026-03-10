#!/usr/bin/env bash
set -euo pipefail

# Build workercmd binaries natively and stage for Dockerfile (for Linux CI)

cargo build --release -p ur-ping -p workercmd-tools -p ur-workerd

DEST=containers/claude-worker
cp target/release/ur-ping "$DEST/ur-ping"
cp target/release/ur-tools "$DEST/ur-tools"
cp target/release/ur-workerd "$DEST/ur-workerd"

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
