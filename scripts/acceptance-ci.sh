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
# A lock dir with no pid file is mid-creation; older than this many minutes, its
# creator died before writing the pid.
LOCK_NO_PID_STALE_MIN=1

# Only remove the lock if we still own it: after a stale takeover, a slow
# previous holder must not delete the new holder's lock on exit.
release_lock() {
    if [ "$(cat "$PID_FILE" 2>/dev/null || true)" = "$$" ]; then
        rm -rf "$LOCK_DIR"
    fi
}

# Take over a stale lock. `mv` is atomic, so when several waiters see the same
# stale lock only one moves it; the others fail the mv and keep waiting. $1 is
# the pid the caller judged dead ("" for a lock with no pid file). If the lock
# was replaced between that check and the mv, we moved a LIVE lock: put it back.
take_over_stale_lock() {
    local expected="$1" moved="$LOCK_DIR.stale.$$" found
    mv "$LOCK_DIR" "$moved" 2>/dev/null || return 0
    found=$(cat "$moved/pid" 2>/dev/null || true)
    if [ "$found" != "$expected" ]; then
        if [ ! -e "$LOCK_DIR" ] && mv "$moved" "$LOCK_DIR" 2>/dev/null; then
            return 0
        fi
        echo "acceptance-ci: lock held by PID ${found:-?} was displaced during stale-lock takeover" >&2
    fi
    rm -rf "$moved"
}

acquire_lock() {
    local err holder lock_branch
    while true; do
        # A failed mkdir with no lock dir is not contention: either the holder
        # released it in between (the retry wins) or mkdir cannot work at all
        # (permissions, read-only fs), which waiting would never fix.
        if err=$(mkdir "$LOCK_DIR" 2>&1) \
            || { [ ! -d "$LOCK_DIR" ] && err=$(mkdir "$LOCK_DIR" 2>&1); }; then
            echo $$ > "$PID_FILE"
            if [ -n "${UR_PUSH_BRANCH:-}" ]; then
                echo "$UR_PUSH_BRANCH" > "$LOCK_DIR/branch"
            fi
            return 0
        fi
        if [ ! -d "$LOCK_DIR" ]; then
            echo "acceptance-ci: cannot create lock $LOCK_DIR: $err" >&2
            exit 1
        fi

        if [ -f "$PID_FILE" ]; then
            holder=$(cat "$PID_FILE" 2>/dev/null || true)
            if [ -n "$holder" ] && ! kill -0 "$holder" 2>/dev/null; then
                take_over_stale_lock "$holder"
                continue
            fi
        elif [ -n "$(find "$LOCK_DIR" -maxdepth 0 -mmin +"$LOCK_NO_PID_STALE_MIN" 2>/dev/null)" ]; then
            take_over_stale_lock ""
            continue
        fi

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

# Fail fast rather than hashing, locking and building against a dead daemon.
docker info >/dev/null 2>&1 || { echo "acceptance-ci: docker unreachable" >&2; exit 1; }

mkdir -p "$(dirname "$LOCK_DIR")"
acquire_lock
trap release_lock EXIT

# Hash under the lock, so the tag names the sources this run actually builds.
TAG="ci-$(scripts/build/image-hash.sh)"
# Log the hash before building: a concurrent pruner sharing this log then sees
# it as the newest entry and keeps it, instead of deleting a build in progress.
record_ci_tag_use "${TAG#ci-}"

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
    rebuilt_tag="ci-$(scripts/build/image-hash.sh)" || {
        remove_ci_tag "${TAG#ci-}"
        echo "acceptance-ci: could not re-hash sources after the build; dropped unverified $TAG images" >&2
        exit 1
    }
    if [ "$rebuilt_tag" != "$TAG" ]; then
        remove_ci_tag "${TAG#ci-}"
        echo "acceptance-ci: sources changed during build ($TAG -> $rebuilt_tag); re-run push" >&2
        exit 1
    fi
fi
prune_old_ci_tags

echo "=== Running acceptance tests with tag=$TAG ==="
UR_IMAGE_TAG="$TAG" cargo test -p acceptance --features acceptance
