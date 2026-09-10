#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/agent-images.sh"

# Build worker binaries natively and stage for Dockerfile (for Linux CI)

cargo build --release -p ur-ping -p workertools -p workerd

# Every agent's base image build context needs its own copy of these
# binaries (each is an independent Docker build context).
for entry in "${AGENT_IMAGES[@]}"; do
    IFS=':' read -r dir _tag _agent _cachebust stage_binaries <<< "$entry"
    [ "$stage_binaries" = "true" ] || continue
    DEST="containers/$dir/bin"
    mkdir -p "$DEST"
    cp target/release/ur-ping "$DEST/ur-ping"
    cp target/release/workertools "$DEST/workertools"
    cp target/release/workerd "$DEST/workerd"
    echo "Staged worker binaries in $DEST/"
done
