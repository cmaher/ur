#!/usr/bin/env bash

# Bookkeeping for content-hash-tagged CI images (`<image>:ci-<hash>`), sourced
# by scripts/acceptance-ci.sh.
#
# Docker's CreatedAt is the original build time, and a cache hit re-tags an old
# image, so "newest by CreatedAt" is not "most recently used" and differs per
# image. Instead every run that uses or builds a hash appends `<epoch> <hash>` to
# a last-used log, and pruning keeps $KEEP_CI_TAGS hashes in total across ALL
# images: the current one plus the most recently used others by that log.
#
# Only hashes that appear in the log are ever pruned. A ci-* tag absent from the
# log belongs to someone else (a push with a different UR_CONFIG, so a different
# log) or predates the log; it is never touched.
#
# Everything here is best effort: a failure warns and never fails the caller.
#
# Callers set: IMAGES (array of image names), TAG (current `ci-<hash>`).
# Optional: KEEP_CI_TAGS (default 3), CI_TAG_LOG (default
# ${UR_CI_STATE_DIR:-$HOME/.ur}/ci-image-tags). The log lives in the host-wide CI
# state dir, not UR_CONFIG: the images are shared by every UR_CONFIG on the host,
# so one log must see every run's use of a hash.

ci_tag_log() {
    echo "${CI_TAG_LOG:-${UR_CI_STATE_DIR:-$HOME/.ur}/ci-image-tags}"
}

# Record that hash ($1, without the `ci-` prefix) was used now.
record_ci_tag_use() {
    local log
    log=$(ci_tag_log)
    if ! { mkdir -p "$(dirname "$log")" && echo "$(date +%s) $1" >> "$log"; } 2>/dev/null; then
        echo "ci-tags: could not update $log" >&2
    fi
}

# Hashes to keep, one per line: $KEEP_CI_TAGS in total, the current one first,
# then the most recently used others by the log. (The caller records the
# current hash before pruning, so it is also the log's newest entry.)
# The limit is applied inside awk, which reads all its input: a `head` would
# exit early and SIGPIPE the pipeline, failing the caller under pipefail. An
# unreadable log yields just the current hash rather than failing the caller.
ci_hashes_to_keep() {
    local log cur="${TAG#ci-}"
    log=$(ci_tag_log)
    echo "$cur"
    if [ -f "$log" ]; then
        LC_ALL=C sort -k1,1nr "$log" 2>/dev/null \
            | awk -v cur="$cur" -v keep="${KEEP_CI_TAGS:-3}" \
                '$2 != cur && !seen[$2]++ && ++n < keep { print $2 }' \
            || true
    fi
}

# Remove every `<image>:ci-<hash>` tag whose hash is in the log but not kept,
# then compact the log to one line per hash that is kept or whose removal failed
# (so it is retried next time rather than leaked).
prune_old_ci_tags() {
    local keep logged img tag hash log retained
    keep=$(ci_hashes_to_keep)
    log=$(ci_tag_log)
    logged=""
    if [ -f "$log" ]; then
        logged=$(awk '{ print $2 }' "$log" 2>/dev/null | LC_ALL=C sort -u || true)
    fi
    retained="$keep"
    for img in "${IMAGES[@]}"; do
        while IFS= read -r tag; do
            hash="${tag#ci-}"
            if grep -qxF "$hash" <<< "$keep" || ! grep -qxF "$hash" <<< "$logged"; then
                continue
            fi
            if ! docker rmi "${img}:${tag}" >/dev/null 2>&1; then
                echo "ci-tags: could not remove ${img}:${tag} (in use?); leaving it" >&2
                retained="$retained"$'\n'"$hash"
            fi
        done < <(docker images --format '{{.Tag}}' "$img" 2>/dev/null | grep '^ci-' || true)
    done

    if [ -f "$log" ]; then
        if ! {
            LC_ALL=C sort -k1,1nr "$log" \
                | awk 'NR == FNR { keep[$1]; next } ($2 in keep) && !seen[$2]++' \
                    <(echo "$retained") - > "$log.tmp" \
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
