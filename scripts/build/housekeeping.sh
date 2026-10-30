#!/usr/bin/env bash
set -uo pipefail

# Reclaim disk from build artifacts. Cargo never evicts stale artifacts and
# BuildKit keeps every layer it has ever built, so both grow without bound.
# Runs after every pre-push (pass or fail); never fails, only warns.
#
#   UR_SWEEP_DAYS          drop target/ artifacts unused for this many days (default 1,
#                          matching ur-hooks/startup-bg/10-cargo-sweep)
#   UR_BUILD_CACHE_LIMIT   cap on Docker's build cache (default 20gb)

days="${UR_SWEEP_DAYS:-1}"
cache_limit="${UR_BUILD_CACHE_LIMIT:-20gb}"

if command -v cargo-sweep >/dev/null 2>&1; then
    cargo sweep --time "$days" >/dev/null 2>&1 \
        || echo "housekeeping: cargo sweep failed" >&2
else
    echo "housekeeping: cargo-sweep not installed; run 'cargo install cargo-sweep' to prune target/" >&2
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
