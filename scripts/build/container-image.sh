#!/usr/bin/env bash
set -euo pipefail

# Build worker container image using the available container runtime.
# Builds ur-worker-base:latest (slow, cached) then ur-worker:latest (fast) on top.

CONTEXT=containers/claude-worker

build_image() {
    local tag="$1"
    local dockerfile="$2"
    echo "Building $tag..."
    if [ "${UR_CONTAINER:-}" = "apple" ] || command -v container >/dev/null 2>&1; then
        ARCH=$(uname -m)
        container build --arch "$ARCH" --tag "$tag" --file "$dockerfile" "$CONTEXT"
    elif command -v docker >/dev/null 2>&1; then
        docker build -t "$tag" -f "$dockerfile" "$CONTEXT"
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl build -t "$tag" -f "$dockerfile" "$CONTEXT"
    else
        echo "Warning: no container runtime found, skipping image build"
        exit 1
    fi
}

build_image ur-worker-base:latest "$CONTEXT/Dockerfile.base"
echo "Base image built: ur-worker-base:latest"

build_image ur-worker:latest "$CONTEXT/Dockerfile"
echo "Worker image built: ur-worker:latest"
