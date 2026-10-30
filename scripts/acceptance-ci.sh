#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/build/agent-images.sh"

# Run acceptance tests against CI images tagged by a content hash of their
# inputs (scripts/build/image-hash.sh). If every image already exists at that
# tag, the release build and `docker build` are skipped entirely; otherwise the
# binaries are staged and the images built. The newest few ci-* tags per image
# are kept so alternating between branches does not rebuild every push.

KEEP_CI_TAGS="${UR_KEEP_CI_TAGS:-3}"
TAG="ci-$(scripts/build/image-hash.sh)"
IMAGES=(ur-worker-base ur-server ur-squid)
for entry in "${AGENT_IMAGES[@]}"; do
    IFS=':' read -r _dir image_tag _agent _cachebust _stage_binaries <<< "$entry"
    IMAGES+=("$image_tag")
done

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
            if [ -n "${UR_PUSH_BRANCH:-}" ]; then
                echo "$UR_PUSH_BRANCH" > "$LOCK_DIR/branch"
            fi
            return 0
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

images_present() {
    local img
    for img in "${IMAGES[@]}"; do
        docker image inspect "${img}:${TAG}" >/dev/null 2>&1 || return 1
    done
}

# Drop all but the newest $KEEP_CI_TAGS ci-* tags of each image (the current
# tag always survives). `docker images` lists newest first.
prune_old_ci_tags() {
    local img old
    for img in "${IMAGES[@]}"; do
        docker images --format '{{.Tag}}' "$img" \
            | { grep '^ci-' || true; } | { grep -vx "$TAG" || true; } \
            | tail -n +"$KEEP_CI_TAGS" \
            | while IFS= read -r old; do
                docker rmi "${img}:${old}" >/dev/null
            done
    done
}

mkdir -p "$(dirname "$LOCK_DIR")"
acquire_lock
trap release_lock EXIT

# An agent update or forced base rebuild must actually rebuild.
if [ -z "${UR_UPDATE_AGENT:-}" ] && [ "${UR_FORCE_REBUILD_BASE:-}" != "1" ] && images_present; then
    echo "=== Images for tag=$TAG already built; skipping build ==="
else
    echo "=== Building images with tag=$TAG ==="
    scripts/build/stage.sh
    UR_IMAGE_TAG="$TAG" scripts/build/image.sh
fi
prune_old_ci_tags

echo "=== Running acceptance tests with tag=$TAG ==="
UR_IMAGE_TAG="$TAG" cargo test -p acceptance --features acceptance
