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

WORKER_CONTEXT=containers/claude-worker
RUST_WORKER_CONTEXT=containers/claude-worker-rust

# Stage vendored mise installer into rust worker build context
cp "$WORKER_CONTEXT/vendor/mise/install.sh" "$RUST_WORKER_CONTEXT/install-mise.sh"

if [ "${UR_FORCE_REBUILD_BASE:-}" = "1" ]; then
    build_image_no_cache "ur-worker-base:$tag" "$WORKER_CONTEXT/Dockerfile.base" "$WORKER_CONTEXT"
else
    build_image "ur-worker-base:$tag" "$WORKER_CONTEXT/Dockerfile.base" "$WORKER_CONTEXT"
fi
echo "Base image built: ur-worker-base:$tag"

if [ "${UR_FORCE_REBUILD_BASE:-}" = "1" ]; then
    build_image_arg "ur-worker:$tag" "$WORKER_CONTEXT/Dockerfile" "$WORKER_CONTEXT" "CACHEBUST=$(date +%s)" "BASE_TAG=$tag"
else
    build_image_arg "ur-worker:$tag" "$WORKER_CONTEXT/Dockerfile" "$WORKER_CONTEXT" "BASE_TAG=$tag"
fi
echo "Worker image built: ur-worker:$tag"

build_image_arg "ur-worker-rust:$tag" "$RUST_WORKER_CONTEXT/Dockerfile" "$RUST_WORKER_CONTEXT" "BASE_TAG=$tag"
echo "Rust worker image built: ur-worker-rust:$tag"

# Download ONNX Runtime if not already cached, then stage into build context.
# The ur-server binary is compiled with ort-load-dynamic and needs libonnxruntime.so.
ORT_VERSION="1.20.0"
ONNX_DIR="${UR_CONFIG:-$HOME/.ur}/onnx"
case "$(uname -m)" in
    arm64|aarch64) ORT_ARCH="aarch64" ;;
    x86_64)        ORT_ARCH="x64" ;;
    *)             echo "Unsupported arch: $(uname -m)" >&2; exit 1 ;;
esac
ORT_SO="$ONNX_DIR/libonnxruntime.so.$ORT_VERSION"
if [ ! -f "$ORT_SO" ]; then
    echo "Downloading ONNX Runtime $ORT_VERSION (linux-$ORT_ARCH)..."
    mkdir -p "$ONNX_DIR"
    curl -fSL "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-linux-${ORT_ARCH}-${ORT_VERSION}.tgz" \
        | tar xz -C /tmp
    cp "/tmp/onnxruntime-linux-${ORT_ARCH}-${ORT_VERSION}/lib/libonnxruntime.so.${ORT_VERSION}" "$ORT_SO"
    rm -rf /tmp/onnxruntime-*
    echo "ONNX Runtime cached at $ONNX_DIR"
fi
cp "$ORT_SO" containers/server/libonnxruntime.so

build_image "ur-server:$tag" containers/server/Dockerfile containers/server
echo "ur-server image built: ur-server:$tag"

build_image "ur-squid:$tag" containers/squid/Dockerfile containers/squid
echo "Squid proxy image built: ur-squid:$tag"
