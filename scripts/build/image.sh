#!/usr/bin/env bash
set -euo pipefail

# Build all container images using Docker (or nerdctl).
# Builds:
#   ur-worker-base:<tag> (slow, cached) + ur-worker:<tag> (fast) on top
#   ur-server:<tag> (Alpine + cross-compiled ur-server binary)
#
# Set UR_IMAGE_TAG to override the default tag (latest).

tag="${UR_IMAGE_TAG:-latest}"

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

build_image_arg() {
    local tag="$1"
    local dockerfile="$2"
    local context="$3"
    shift 3
    local args=()
    for arg in "$@"; do
        args+=(--build-arg "$arg")
    done
    echo "Building $tag..."
    if command -v docker >/dev/null 2>&1; then
        docker build "${args[@]}" -t "$tag" -f "$dockerfile" "$context"
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl build "${args[@]}" -t "$tag" -f "$dockerfile" "$context"
    else
        echo "Warning: no container runtime found, skipping image build" >&2
        exit 1
    fi
}

build_image_no_cache() {
    local tag="$1"
    local dockerfile="$2"
    local context="$3"
    echo "Building $tag (no cache)..."
    if command -v docker >/dev/null 2>&1; then
        docker build --no-cache -t "$tag" -f "$dockerfile" "$context"
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl build --no-cache -t "$tag" -f "$dockerfile" "$context"
    else
        echo "Warning: no container runtime found, skipping image build" >&2
        exit 1
    fi
}

BASE_CONTEXT=containers/worker-base
WORKER_CONTEXT=containers/agent-claude
RUST_WORKER_CONTEXT=containers/agent-claude-rust

# Stage vendored mise installer into the rust worker build context
cp "$BASE_CONTEXT/vendor/mise/install.sh" "$RUST_WORKER_CONTEXT/install-mise.sh"

# `ur-worker-base` now carries only agent-agnostic assets (shell setup, skills,
# instructions) — no Claude CLI install — so UR_FORCE_REBUILD_BASE no longer
# touches the Claude install at all. It still fully rebuilds this layer.
if [ "${UR_FORCE_REBUILD_BASE:-}" = "1" ]; then
    build_image_no_cache "ur-worker-base:$tag" "$BASE_CONTEXT/Dockerfile" "$BASE_CONTEXT"
else
    build_image "ur-worker-base:$tag" "$BASE_CONTEXT/Dockerfile" "$BASE_CONTEXT"
fi
echo "Base image built: ur-worker-base:$tag"

# The Claude CLI install (and its `claude update` layer) now lives in the
# agent-claude image, not the base. The worker image's `claude update` layer
# is cached like any other, so Claude Code stays pinned at the version baked
# in until CACHEBUST changes. Both UR_FORCE_REBUILD_BASE=1 (full base
# rebuild, which invalidates this layer's cache since it's FROM the base) and
# UR_UPDATE_CLAUDE=1 (cheap: just this layer) bust it.
worker_args=("BASE_TAG=$tag")
if [ "${UR_FORCE_REBUILD_BASE:-}" = "1" ] || [ "${UR_UPDATE_CLAUDE:-}" = "1" ]; then
    worker_args+=("CACHEBUST=$(date +%s)")
fi
build_image_arg "ur-worker:$tag" "$WORKER_CONTEXT/Dockerfile" "$WORKER_CONTEXT" "${worker_args[@]}"
echo "Worker image built: ur-worker:$tag"

build_image_arg "ur-worker-rust:$tag" "$RUST_WORKER_CONTEXT/Dockerfile" "$RUST_WORKER_CONTEXT" "BASE_TAG=$tag"
echo "Rust worker image built: ur-worker-rust:$tag"

build_image "ur-server:$tag" containers/server/Dockerfile containers/server
echo "ur-server image built: ur-server:$tag"

build_image "ur-squid:$tag" containers/squid/Dockerfile containers/squid
echo "Squid proxy image built: ur-squid:$tag"
