#!/usr/bin/env bash
set -euo pipefail

# Build worker binaries natively and stage for Dockerfile (for Linux CI)

cargo build --release -p ur-ping -p workertools -p workerd

# Every agent's base image build context needs its own copy of these
# binaries (each is an independent Docker build context).
for DEST in containers/worker-claude/bin containers/worker-codex/bin; do
    mkdir -p "$DEST"
    cp target/release/ur-ping "$DEST/ur-ping"
    cp target/release/workertools "$DEST/workertools"
    cp target/release/workerd "$DEST/workerd"
    echo "Staged worker binaries in $DEST/"
done
