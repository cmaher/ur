#!/usr/bin/env bash
# shellcheck source-path=SCRIPTDIR
set -euo pipefail

source "$(dirname "$0")/build/agent-images.sh"
source "$(dirname "$0")/build/ci-tags.sh"
# Mutex: only one acceptance-ci / pre-push runs image builds at a time.
source "$(dirname "$0")/build/ci-lock.sh"

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

images_present() {
    local img
    for img in "${IMAGES[@]}"; do
        docker image inspect "${img}:${TAG}" >/dev/null 2>&1 || return 1
    done
}

# Fail fast rather than hashing, locking and building against a dead daemon.
docker info >/dev/null 2>&1 || { echo "acceptance-ci: docker unreachable" >&2; exit 1; }

mkdir -p "$LOCKS_DIR"
acquire_lock
trap release_lock EXIT
# From here on fd 9 holds the lock (scripts/build/ci-lock.sh). Every child that
# can outlive this script (builds, the acceptance suite and anything it starts)
# runs with `9>&-`, so a leftover process never keeps the lock held.

# Hash under the lock, so the tag names the sources this run actually builds.
TAG="ci-$(scripts/build/image-hash.sh 9>&-)"
# Log the hash before building: a concurrent pruner sharing this log then sees
# it as the newest entry and keeps it, instead of deleting a build in progress.
record_ci_tag_use "${TAG#ci-}"

# An agent update or forced base rebuild must actually rebuild.
if [ -z "${UR_UPDATE_AGENT:-}" ] && [ "${UR_FORCE_REBUILD_BASE:-}" != "1" ] && images_present; then
    echo "=== Images for tag=$TAG already built; skipping build ==="
else
    echo "=== Building images with tag=$TAG ==="
    scripts/build/stage.sh 9>&-
    UR_IMAGE_TAG="$TAG" scripts/build/image.sh 9>&-
    # A source edit during the build would leave images tagged with a hash
    # they do not match; a later push with those sources would then skip the
    # build and test stale images. Drop them and fail instead.
    rebuilt_tag="ci-$(scripts/build/image-hash.sh 9>&-)" || {
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
UR_IMAGE_TAG="$TAG" cargo test -p acceptance --features acceptance 9>&-
