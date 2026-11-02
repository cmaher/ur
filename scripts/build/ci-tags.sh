#!/usr/bin/env bash

# Bookkeeping for content-hash-tagged CI images (`<image>:ci-<hash>`), sourced
# by scripts/acceptance-ci.sh.
#
# Docker's CreatedAt is the original build time, and a cache hit re-tags an old
# image, so "newest by CreatedAt" is not "most recently used" and differs per
# image. Instead every run that uses or builds a hash appends `<epoch> <hash>` to
# a last-used log, and pruning keeps the newest $KEEP_CI_TAGS hashes by that log
# across ALL images (plus the current one). ci-* tags whose hash is not in the
# log predate it and are pruned too.
#
# Everything here is best effort: a failure warns and never fails the caller.
#
# Callers set: IMAGES (array of image names), TAG (current `ci-<hash>`).
# Optional: KEEP_CI_TAGS (default 3), CI_TAG_LOG (default
# ${UR_CONFIG:-$HOME/.ur}/ci-image-tags).

ci_tag_log() {
    echo "${CI_TAG_LOG:-${UR_CONFIG:-$HOME/.ur}/ci-image-tags}"
}

# Record that hash ($1, without the `ci-` prefix) was used now.
record_ci_tag_use() {
    local log
    log=$(ci_tag_log)
    if ! { mkdir -p "$(dirname "$log")" && echo "$(date +%s) $1" >> "$log"; } 2>/dev/null; then
        echo "ci-tags: could not update $log" >&2
    fi
}

# Hashes to keep, one per line: the newest $KEEP_CI_TAGS distinct hashes by last
# use, plus the current one.
ci_hashes_to_keep() {
    local log
    log=$(ci_tag_log)
    echo "${TAG#ci-}"
    if [ -f "$log" ]; then
        LC_ALL=C sort -k1,1nr "$log" 2>/dev/null \
            | awk '!seen[$2]++ { print $2 }' \
            | head -n "${KEEP_CI_TAGS:-3}"
    fi
}

# Remove every `<image>:ci-<hash>` tag whose hash is not kept, and compact the
# log to one line per kept hash.
prune_old_ci_tags() {
    local keep img tag log
    keep=$(ci_hashes_to_keep)
    for img in "${IMAGES[@]}"; do
        while IFS= read -r tag; do
            if ! grep -qxF "${tag#ci-}" <<< "$keep"; then
                docker rmi "${img}:${tag}" >/dev/null 2>&1 \
                    || echo "ci-tags: could not remove ${img}:${tag} (in use?); leaving it" >&2
            fi
        done < <(docker images --format '{{.Tag}}' "$img" 2>/dev/null | grep '^ci-' || true)
    done

    log=$(ci_tag_log)
    if [ -f "$log" ]; then
        if ! {
            LC_ALL=C sort -k1,1nr "$log" \
                | awk 'NR == FNR { keep[$1]; next } ($2 in keep) && !seen[$2]++' \
                    <(echo "$keep") - > "$log.tmp" \
                && mv "$log.tmp" "$log"
        } 2>/dev/null; then
            rm -f "$log.tmp"
            echo "ci-tags: could not compact $log" >&2
        fi
    fi
}

# Best-effort removal of every image at `ci-<hash>` ($1, without the prefix).
remove_ci_tag() {
    local img
    for img in "${IMAGES[@]}"; do
        docker rmi "${img}:ci-$1" >/dev/null 2>&1 || true
    done
}
