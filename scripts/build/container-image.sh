#!/usr/bin/env bash
set -euo pipefail

# Build all container images using Docker (or nerdctl).
# Builds:
#   ur-worker-base:latest (slow, cached) + ur-worker:latest (fast) on top
#   ur-server:latest (Alpine + cross-compiled ur-server binary)

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
RUST_WORKER_CONTEXT=containers/claude-worker-rust

# Download installer scripts if not present (gitignored, cached locally)
CLAUDE_INSTALLER="$WORKER_CONTEXT/install-claude.sh"
MISE_INSTALLER="$RUST_WORKER_CONTEXT/install-mise.sh"

if [ ! -f "$CLAUDE_INSTALLER" ]; then
    echo "Downloading Claude Code installer..."
    curl -fsSL -o "$CLAUDE_INSTALLER" https://storage.googleapis.com/anthropic-claude-code/claude-code-installer.sh
fi

if [ ! -f "$MISE_INSTALLER" ]; then
    echo "Downloading mise installer..."
    curl -fsSL -o "$MISE_INSTALLER" https://mise.jdx.dev/install.sh
fi

build_image ur-worker-base:latest "$WORKER_CONTEXT/Dockerfile.base" "$WORKER_CONTEXT"
echo "Base image built: ur-worker-base:latest"

build_image ur-worker:latest "$WORKER_CONTEXT/Dockerfile" "$WORKER_CONTEXT"
echo "Worker image built: ur-worker:latest"

build_image ur-worker-rust:latest "$RUST_WORKER_CONTEXT/Dockerfile" "$RUST_WORKER_CONTEXT"
echo "Rust worker image built: ur-worker-rust:latest"

build_image ur-server:latest containers/server/Dockerfile containers/server
echo "ur-server image built: ur-server:latest"

build_image ur-squid:latest containers/squid/Dockerfile containers/squid
echo "Squid proxy image built: ur-squid:latest"
