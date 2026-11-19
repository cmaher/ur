#!/usr/bin/env bash
set -euo pipefail

# Rebuild and redeploy the ur-server container.
# Stages all image binaries (scripts/build/stage.sh: ur-server and the worker
# binaries, in one build), rebuilds the ur-server image, and recreates the
# container.

echo "=== Staging image binaries ==="
scripts/build/stage.sh

echo ""
echo "=== Building ur-server image ==="
if command -v docker >/dev/null 2>&1; then
    RUNTIME=docker
elif command -v nerdctl >/dev/null 2>&1; then
    RUNTIME=nerdctl
else
    echo "No container runtime found" >&2
    exit 1
fi
$RUNTIME build -t ur-server:latest -f containers/server/Dockerfile containers/server

echo ""
echo "=== Recreating ur-server container ==="
# Recreate through ur rather than raw `docker compose`: ur supplies the
# the host-resolved UR_MACHINE_ID and UR_HOST_HOME the server needs.
ur server redeploy server
