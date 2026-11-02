#!/usr/bin/env bash
set -uo pipefail

# Reclaim disk from build artifacts. Cargo never evicts stale artifacts and
# BuildKit keeps every layer it has ever built, so both grow without bound.
# Runs after every pre-push (pass or fail); never fails, only warns.
#
#   UR_SWEEP_DAYS          drop target/ artifacts unused for this many days (default 7)
#   UR_BUILD_CACHE_LIMIT   cap on Docker's build cache (default 20gb)
#
# `cargo sweep --time N` judges "unused" by file access time. macOS (APFS) and
# Linux relatime only update atime lazily, so a recently used artifact can look
# as old as its build; a short window would delete live artifacts and force
# rebuilds. A week is long enough to be safe.

days="${UR_SWEEP_DAYS:-7}"
cache_limit="${UR_BUILD_CACHE_LIMIT:-20gb}"

# True if a cargo build holds a lock in target/ right now (cargo flocks
# target/<profile>/.cargo-lock and target/<triple>/<profile>/.cargo-lock while
# building). One non-blocking shared-lock attempt per lock file.
cargo_build_running() {
    local tdir="${CARGO_TARGET_DIR:-target}"
    perl -MFcntl=:flock -e 'for (@ARGV) { open(my $f, "<", $_) or next;
                                         flock($f, LOCK_SH | LOCK_NB) or exit 0 }
                            exit 1' \
        "$tdir"/*/.cargo-lock "$tdir"/*/*/.cargo-lock 2>/dev/null
}

if ! command -v cargo-sweep >/dev/null 2>&1; then
    echo "housekeeping: cargo-sweep not installed; run 'cargo install cargo-sweep' to prune target/" >&2
elif cargo_build_running; then
    # Sweeping under a running build can delete artifacts it is about to use.
    echo "housekeeping: a cargo build is running in target/; skipping cargo sweep" >&2
elif ! cargo sweep --time "$days" >/dev/null 2>&1; then
    echo "housekeeping: cargo sweep failed" >&2
fi

if command -v docker >/dev/null 2>&1; then
    # --max-used-space replaced --keep-storage in newer Docker releases.
    docker builder prune -f --max-used-space "$cache_limit" >/dev/null 2>&1 \
        || docker builder prune -f --keep-storage "$cache_limit" >/dev/null 2>&1 \
        || echo "housekeeping: docker builder prune failed" >&2
    # Untagged images left behind when a tag (e.g. :latest) moves to a new build.
    docker image prune -f >/dev/null 2>&1 \
        || echo "housekeeping: docker image prune failed" >&2
fi

exit 0
