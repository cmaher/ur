#!/usr/bin/env bash
set -euo pipefail

# Build worker binaries natively and stage for Dockerfile (for Linux CI)

cargo build --release -p ur-ping -p workertools -p workerd

DEST=containers/claude-worker/bin
mkdir -p "$DEST"
cp target/release/ur-ping "$DEST/ur-ping"
cp target/release/workertools "$DEST/workertools"
cp target/release/workerd "$DEST/workerd"

echo "Staged worker binaries in $DEST/"
