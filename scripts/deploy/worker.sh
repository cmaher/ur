#!/usr/bin/env bash
set -euo pipefail

# Rebuild and redeploy a worker container.
# Stages workercmd binaries, rebuilds worker images, and relaunches the worker.
#
# Usage: scripts/deploy/worker.sh [PROCESS_ID]
#   PROCESS_ID defaults to "ur"

PROCESS_ID="${1:-ur}"

echo "=== Staging workercmd binaries ==="
scripts/build/stage-workercmd.sh

echo ""
echo "=== Building worker images ==="
if command -v docker >/dev/null 2>&1; then
    RUNTIME=docker
elif command -v nerdctl >/dev/null 2>&1; then
    RUNTIME=nerdctl
else
    echo "No container runtime found" >&2
    exit 1
fi

WORKER_CONTEXT=containers/claude-worker
RUST_WORKER_CONTEXT=containers/claude-worker-rust

$RUNTIME build -t ur-worker:latest -f "$WORKER_CONTEXT/Dockerfile" "$WORKER_CONTEXT"
$RUNTIME build -t ur-worker-rust:latest -f "$RUST_WORKER_CONTEXT/Dockerfile" "$RUST_WORKER_CONTEXT"

echo ""
echo "=== Relaunching worker $PROCESS_ID ==="
UR_BIN="${UR_BIN:-target/debug/ur}"
"$UR_BIN" process kill "$PROCESS_ID" 2>/dev/null || true
"$UR_BIN" process launch "$PROCESS_ID"

echo "Worker $PROCESS_ID redeployed"
