#!/usr/bin/env bash
set -euo pipefail

# Build worker binaries natively and stage for Dockerfile (for Linux CI)

cargo build --release -p ur-ping -p workertools -p workerd -p ur-osc8

DEST=containers/claude-worker/bin
mkdir -p "$DEST"
cp target/release/ur-ping "$DEST/ur-ping"
cp target/release/workertools "$DEST/workertools"
cp target/release/workerd "$DEST/workerd"
cp target/release/ur-osc8 "$DEST/ur-osc8"

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
