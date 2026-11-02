#!/usr/bin/env bash
# shellcheck source-path=SCRIPTDIR
set -euo pipefail

source "$(dirname "$0")/build/agent-images.sh"
source "$(dirname "$0")/build/ci-tags.sh"

# Run acceptance tests against CI images tagged by a content hash of their
# inputs (scripts/build/image-hash.sh). If every image already exists at that
# tag, the release build and `docker build` are skipped entirely; otherwise the
# binaries are staged and the images built. The ci-* tags of the few most
# recently used hashes are kept (scripts/build/ci-tags.sh) so alternating
# between branches does not rebuild every push.

KEEP_CI_TAGS="${UR_KEEP_CI_TAGS:-3}"
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

mkdir -p "$(dirname "$LOCK_DIR")"
acquire_lock
trap release_lock EXIT

# Hash under the lock, so the tag names the sources this run actually builds.
TAG="ci-$(scripts/build/image-hash.sh)"

# An agent update or forced base rebuild must actually rebuild.
if [ -z "${UR_UPDATE_AGENT:-}" ] && [ "${UR_FORCE_REBUILD_BASE:-}" != "1" ] && images_present; then
    echo "=== Images for tag=$TAG already built; skipping build ==="
else
    echo "=== Building images with tag=$TAG ==="
    scripts/build/stage.sh
    UR_IMAGE_TAG="$TAG" scripts/build/image.sh
    # A source edit during the build would leave images tagged with a hash
    # they do not match; a later push with those sources would then skip the
    # build and test stale images. Drop them and fail instead.
    rebuilt_tag="ci-$(scripts/build/image-hash.sh)"
    if [ "$rebuilt_tag" != "$TAG" ]; then
        remove_ci_tag "${TAG#ci-}"
        echo "acceptance-ci: sources changed during build ($TAG -> $rebuilt_tag); re-run push" >&2
        exit 1
    fi
fi
record_ci_tag_use "${TAG#ci-}"
prune_old_ci_tags

echo "=== Running acceptance tests with tag=$TAG ==="
UR_IMAGE_TAG="$TAG" cargo test -p acceptance --features acceptance
