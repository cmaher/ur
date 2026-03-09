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

# Build base image only if it doesn't exist or --rebuild-base is passed
REBUILD_BASE=false
if [ "${1:-}" = "--rebuild-base" ]; then
    REBUILD_BASE=true
    shift
fi

has_base_image() {
    if [ "${UR_CONTAINER:-}" = "apple" ] || command -v container >/dev/null 2>&1; then
        container image inspect ur-worker-base:latest >/dev/null 2>&1
    elif command -v docker >/dev/null 2>&1; then
        docker image inspect ur-worker-base:latest >/dev/null 2>&1
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl image inspect ur-worker-base:latest >/dev/null 2>&1
    else
        return 1
    fi
}

if [ "$REBUILD_BASE" = true ] || ! has_base_image; then
    build_image ur-worker-base:latest "$CONTEXT/Dockerfile.base"
    echo "Base image built: ur-worker-base:latest"
else
    echo "Base image ur-worker-base:latest exists, skipping (use --rebuild-base to force)"
fi

build_image ur-worker:latest "$CONTEXT/Dockerfile"
echo "Worker image built: ur-worker:latest"
