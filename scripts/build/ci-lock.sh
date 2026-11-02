#!/usr/bin/env bash

# Mutex for scripts/acceptance-ci.sh: only one image build + acceptance run at a
# time per UR_CONFIG. Sourced; defines LOCK, acquire_lock, release_lock.
#
# The lock is a symlink whose target is `<pid>:<branch>`. symlink(2) creates it
# atomically and only if nothing is there, so the lock and its owner's pid
# appear in ONE step: there is no window in which a lock exists without a pid
# for another waiter to misjudge as stale. (A directory + pid file needs two
# steps, and a waiter can delete a live lock in between.)
#
# Stale locks (owner pid not running) are taken over with an atomic rename of
# the symlink, so when several waiters see the same stale lock only one wins.
#
# Optional: UR_LOCK_POLL_SECS (default 10) between checks while waiting.

LOCK="${UR_CONFIG:-$HOME/.ur}/locks/pre-push.lock"

# Remove the lock only if we still own it: after a stale takeover, a slow
# previous holder must not delete the new holder's lock on exit.
release_lock() {
    local target
    target=$(readlink "$LOCK" 2>/dev/null) || return 0
    if [ "${target%%:*}" = "$$" ]; then
        rm -f "$LOCK"
    fi
}

# Take over a stale lock whose target the caller read as $1. Only one waiter's
# rename succeeds; the others find nothing to move and retry. If the lock was
# replaced between the caller's read and the rename, we moved a LIVE lock: put
# an identical one back (ln is create-if-absent, so this never clobbers).
take_over_stale_lock() {
    local observed="$1" moved="$LOCK.stale.$$" found
    mv "$LOCK" "$moved" 2>/dev/null || return 0
    found=$(readlink "$moved" 2>/dev/null || true)
    if [ "$found" != "$observed" ]; then
        ln -sn "$found" "$LOCK" 2>/dev/null \
            || echo "acceptance-ci: live lock ($found) was displaced during stale-lock takeover" >&2
    fi
    rm -f "$moved"
}

acquire_lock() {
    local mine="$$:${UR_PUSH_BRANCH:-?}" err target pid retried=""
    if [ -d "$LOCK" ] && [ ! -L "$LOCK" ]; then
        # ln would create the link INSIDE a directory and appear to succeed.
        echo "acceptance-ci: $LOCK is a directory, not a lock; remove it" >&2
        exit 1
    fi
    while true; do
        if err=$(ln -sn "$mine" "$LOCK" 2>&1); then
            return 0
        fi
        if ! target=$(readlink "$LOCK" 2>/dev/null); then
            # Nothing to wait for. Either the holder released between our ln
            # and readlink (retry once), or ln cannot work here (permissions,
            # read-only fs, a non-symlink in the way), which waiting never fixes.
            if [ -z "$retried" ] && [ ! -e "$LOCK" ]; then
                retried=1
                continue
            fi
            echo "acceptance-ci: cannot create lock $LOCK: $err" >&2
            exit 1
        fi
        retried=""
        pid="${target%%:*}"
        if ! kill -0 "$pid" 2>/dev/null; then
            take_over_stale_lock "$target"
            continue
        fi
        echo "acceptance-ci: waiting for lock (PID $pid, branch ${target#*:})..."
        sleep "${UR_LOCK_POLL_SECS:-10}"
    done
}
