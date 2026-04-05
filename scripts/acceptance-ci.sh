#!/usr/bin/env bash
set -euo pipefail

# Run acceptance tests with isolated CI-tagged images.
# Builds all 5 images with a unique ci-<label> tag, runs acceptance tests,
# then cleans up the tagged images (even on failure).

LABEL=$(od -An -tx1 -N5 /dev/urandom | tr -d ' \n' | cut -c1-5)
TAG="ci-${LABEL}"
IMAGES=(ur-worker-base ur-worker ur-worker-rust ur-server ur-squid)

# --- Mutex: only one acceptance-ci / pre-push runs image builds at a time ---
LOCK_DIR="${UR_CONFIG:-$HOME/.ur}/locks/pre-push"
PID_FILE="$LOCK_DIR/pid"

release_lock() {
    rm -rf "$LOCK_DIR"
}

acquire_lock() {
    while true; do
        if mkdir "$LOCK_DIR" 2>/dev/null; then
            echo $$ > "$PID_FILE"
            [ -n "${UR_PUSH_BRANCH:-}" ] && echo "$UR_PUSH_BRANCH" > "$LOCK_DIR/branch"
            return
        fi

        if [ -f "$PID_FILE" ]; then
            local stale_pid
            stale_pid=$(cat "$PID_FILE" 2>/dev/null || true)
            if [ -n "$stale_pid" ] && ! kill -0 "$stale_pid" 2>/dev/null; then
                rm -rf "$LOCK_DIR"
                continue
            fi
        fi

        local lock_branch
        lock_branch=$(cat "$LOCK_DIR/branch" 2>/dev/null || echo '?')
        echo "acceptance-ci: waiting for lock (PID $(cat "$PID_FILE" 2>/dev/null || echo '?'), branch $lock_branch)..."
        sleep 10
    done
}

cleanup() {
    echo "Cleaning up ci-tagged images (tag=$TAG)..."
    for img in "${IMAGES[@]}"; do
        docker rmi "${img}:${TAG}" 2>/dev/null || true
    done
    release_lock
}

mkdir -p "$(dirname "$LOCK_DIR")"
acquire_lock
trap cleanup EXIT

echo "=== Building images with tag=$TAG ==="
UR_IMAGE_TAG="$TAG" scripts/build/image.sh >/dev/null

echo "=== Running acceptance tests with tag=$TAG ==="
UR_IMAGE_TAG="$TAG" cargo test -p acceptance --features acceptance
