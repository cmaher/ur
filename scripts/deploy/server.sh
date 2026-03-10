#!/usr/bin/env bash
set -euo pipefail

# Rebuild and redeploy the ur-server container.
# Stages the server binary, rebuilds the image, and recreates the container.

echo "=== Staging ur-server binary ==="
scripts/build/stage-server.sh

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
# docker compose up -d will recreate only containers whose image changed
COMPOSE_FILE="${COMPOSE_FILE:-$HOME/.ur/docker-compose.yml}"
if [ -f "$COMPOSE_FILE" ]; then
    $RUNTIME compose -f "$COMPOSE_FILE" up -d ur-server
    echo "ur-server container recreated"
else
    echo "No compose file at $COMPOSE_FILE — run 'ur start' to generate it"
    exit 1
fi
