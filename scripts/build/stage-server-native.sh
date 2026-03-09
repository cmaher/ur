#!/usr/bin/env bash
set -euo pipefail

# Build ur-server binary natively and stage for Dockerfile (for Linux CI).

cargo build --release -p ur-server

DEST=containers/server
cp target/release/ur-server "$DEST/ur-server"

echo "Staged ur-server binary in $DEST/"
