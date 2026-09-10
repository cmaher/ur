#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/../build/agent-images.sh"

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

for entry in "${AGENT_IMAGES[@]}"; do
    IFS=':' read -r dir image_tag _agent _cachebust _stage_binaries <<< "$entry"
    context="containers/$dir"
    $RUNTIME build --build-arg BASE_TAG=latest -t "$image_tag:latest" \
        -f "$context/Dockerfile" "$context"
done

echo ""
echo "=== Relaunching worker $PROCESS_ID ==="
UR_BIN="${UR_BIN:-target/debug/ur}"
"$UR_BIN" process kill "$PROCESS_ID" 2>/dev/null || true
"$UR_BIN" process launch "$PROCESS_ID"

echo "Worker $PROCESS_ID redeployed"
