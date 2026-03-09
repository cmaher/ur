#!/usr/bin/env bash
set -euo pipefail

# Build all container images using Docker (or nerdctl).
# Builds:
#   ur-worker-base:latest (slow, cached) + ur-worker:latest (fast) on top
#   ur-server:latest (Alpine + cross-compiled urd binary)

build_image() {
    local tag="$1"
    local dockerfile="$2"
    local context="$3"
    echo "Building $tag..."
    if command -v docker >/dev/null 2>&1; then
        docker build -t "$tag" -f "$dockerfile" "$context"
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl build -t "$tag" -f "$dockerfile" "$context"
    else
        echo "Warning: no container runtime found, skipping image build" >&2
        exit 1
    fi
}

WORKER_CONTEXT=containers/claude-worker

build_image ur-worker-base:latest "$WORKER_CONTEXT/Dockerfile.base" "$WORKER_CONTEXT"
echo "Base image built: ur-worker-base:latest"

build_image ur-worker:latest "$WORKER_CONTEXT/Dockerfile" "$WORKER_CONTEXT"
echo "Worker image built: ur-worker:latest"

build_image ur-server:latest containers/server/Dockerfile containers/server
echo "urd image built: ur-server:latest"
