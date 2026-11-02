#!/usr/bin/env bash

# Mutex for scripts/acceptance-ci.sh: only one image build + acceptance run at a
# time per UR_CONFIG. Sourced; defines acquire_lock and release_lock and uses
# file descriptor 9. Needs perl (for flock).
#
# The lock is a kernel flock(LOCK_EX) on $LOCK_FILE. The shell opens the file on
# fd 9 and a one-shot perl takes the flock on that inherited fd. A flock belongs
# to the open file description, which the shell keeps after perl exits, so the
# lock is held until the shell exits, and the kernel releases it if the shell
# dies (even on SIGKILL). There is no stale-lock detection or takeover to get
# wrong. Any child that also inherits fd 9 keeps the lock alive while it runs,
# so long-lived children must be started with `9>&-`.
#
# Optional: UR_LOCK_POLL_SECS (default 10) between attempts while waiting.

LOCKS_DIR="${UR_CONFIG:-$HOME/.ur}/locks"
LOCK_FILE="$LOCKS_DIR/pre-push.flock"
# `<pid>:<branch>` of the current holder, for the waiting message only.
LOCK_HOLDER="$LOCKS_DIR/pre-push.holder"

# One non-blocking attempt on fd 9: 0 = acquired, 1 = held by someone else,
# anything else = error.
try_flock() {
    perl -MFcntl=:flock -e 'open(my $fh, ">&=", 9) or die "flock: fd 9: $!\n";
                            flock($fh, LOCK_NB | LOCK_EX) and exit 0;
                            $!{EWOULDBLOCK} and exit 1; die "flock: $!\n"'
}

acquire_lock() {
    local rc holder
    if ! (: >> "$LOCK_FILE") 2>/dev/null; then
        echo "acceptance-ci: cannot open lock file $LOCK_FILE (is $LOCKS_DIR writable?)" >&2
        exit 1
    fi
    exec 9>>"$LOCK_FILE"
    while true; do
        if try_flock; then
            break
        else
            rc=$?
        fi
        if [ "$rc" -ne 1 ]; then
            echo "acceptance-ci: cannot lock $LOCK_FILE" >&2
            exit 1
        fi
        holder=$(cat "$LOCK_HOLDER" 2>/dev/null || echo '?:?')
        echo "acceptance-ci: waiting for lock (PID ${holder%%:*}, branch ${holder#*:})..."
        sleep "${UR_LOCK_POLL_SECS:-10}" 9>&-
    done
    echo "$$:${UR_PUSH_BRANCH:-?}" > "$LOCK_HOLDER" 2>/dev/null || true
    acquire_legacy_lock
}

release_lock() {
    release_legacy_lock
    if [ "$(cat "$LOCK_HOLDER" 2>/dev/null || true)" = "$$:${UR_PUSH_BRANCH:-?}" ]; then
        rm -f "$LOCK_HOLDER"
    fi
    exec 9>&-
}

# --- LEGACY compat: remove once no branch runs a pre-flock acceptance-ci.sh ---
# Older scripts (master before ur-1pijn) lock the DIRECTORY locks/pre-push with
# a `pid` file inside and take it over when `kill -0 <pid>` fails. While holding
# the flock we also hold that directory, so old and new scripts exclude each
# other. Only old scripts can contend for it here (new ones are serialized by
# the flock), and they see our live pid and wait.
LEGACY_LOCK="$LOCKS_DIR/pre-push"

acquire_legacy_lock() {
    local pid
    while ! mkdir "$LEGACY_LOCK" 2>/dev/null; do
        if [ ! -d "$LEGACY_LOCK" ]; then
            echo "acceptance-ci: cannot create legacy lock $LEGACY_LOCK" >&2
            exit 1
        fi
        pid=$(cat "$LEGACY_LOCK/pid" 2>/dev/null || true)
        if [ -z "$pid" ]; then
            # An old script writes its pid right after mkdir: give it a moment.
            sleep 1 9>&-
            pid=$(cat "$LEGACY_LOCK/pid" 2>/dev/null || true)
        fi
        if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then
            echo "acceptance-ci: waiting for lock held by an older acceptance-ci (PID $pid)..."
            sleep "${UR_LOCK_POLL_SECS:-10}" 9>&-
            continue
        fi
        rm -rf "$LEGACY_LOCK"
    done
    echo $$ > "$LEGACY_LOCK/pid"
    if [ -n "${UR_PUSH_BRANCH:-}" ]; then
        echo "$UR_PUSH_BRANCH" > "$LEGACY_LOCK/branch"
    fi
}

release_legacy_lock() {
    if [ "$(cat "$LEGACY_LOCK/pid" 2>/dev/null || true)" = "$$" ]; then
        rm -rf "$LEGACY_LOCK"
    fi
}
# --- end LEGACY compat ---
