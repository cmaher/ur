#!/usr/bin/env bash
# Tests for the CI shell layer: image-hash.sh, acceptance-ci.sh (+ ci-tags.sh,
# ci-lock.sh), stage.sh, housekeeping.sh and the pre-push hook.
#
#   scripts/build/test-ci-scripts.sh [TEST_NAME_REGEX]
#
# Every test runs the real scripts in a throwaway git repo under a mktemp dir,
# with docker, cargo and rustc stubbed, HOME and UR_CONFIG pointed into the
# sandbox, and git fault injection available via a pass-through wrapper. It
# never touches real docker, images, ~/.ur or the real acceptance lock.
#
# UR_CI_TEST_SRC=<dir> tests the scripts under <dir> instead of this checkout
# (used to run mutants against the suite). Needs bash 3.2+, git, a sha256 tool,
# and `timeout` or perl. Runs serially; each script invocation gets a generous
# timeout (UR_CI_TEST_STEP_TIMEOUT, default 120s) so a loaded machine slows the
# suite rather than flaking it. UR_CI_TEST_KEEP=1 keeps the sandbox for debugging;
# UR_CI_TEST_FAIL_FAST=1 stops at the first failing test (mutation runs).

# Test hooks (DOCKER_BUILD_HOOK, CARGO_TEST_HOOK) are single-quoted on purpose:
# the stubs eval them inside the script under test.
# shellcheck disable=SC2016
set -u

SRC="${UR_CI_TEST_SRC:-$(cd "$(dirname "$0")/../.." && pwd -P)}"
ONLY="${1:-.}"
STEP_TIMEOUT="${UR_CI_TEST_STEP_TIMEOUT:-120}"

# Hooks export repo-local git variables (GIT_DIR, ...); they must not leak into
# the sandbox repos.
# shellcheck disable=SC2046
unset $(git rev-parse --local-env-vars 2>/dev/null)

if [ -x /usr/bin/git ]; then REAL_GIT=/usr/bin/git; else REAL_GIT=$(command -v git); fi
REAL_LN=$(command -v ln)
if command -v timeout >/dev/null 2>&1; then
    TO=(timeout "$STEP_TIMEOUT")
else
    TO=(perl -e 'alarm shift; exec @ARGV or die "exec: $!"' "$STEP_TIMEOUT")
fi

S=$(mktemp -d "${TMPDIR:-/tmp}/test-ci-scripts.XXXXXX")
S=$(cd "$S" && pwd -P)
BGPIDS=()
kill_bg() {
    local p
    for p in "${BGPIDS[@]:-}"; do [ -n "$p" ] && kill "$p" 2>/dev/null; done
    return 0
}
cleanup() {
    chmod -R u+w "$S" 2>/dev/null
    rm -rf "$S"
}
[ -n "${UR_CI_TEST_KEEP:-}" ] || trap cleanup EXIT

STUBS="$S/stubs"
mkdir -p "$STUBS" "$S/home"
SANDBOX_HOME="$S/home"
BASEPATH="$STUBS:/usr/bin:/bin:/usr/sbin:/sbin"

# ---------------------------------------------------------------- stubs ----
# docker: a flat "image:tag" store in $DSTATE/images; $DSTATE/inuse lists refs
# that `rmi` refuses (held by a container).
cat > "$STUBS/docker" <<'EOF'
#!/usr/bin/env bash
touch "$DSTATE/images" "$DSTATE/inuse"
echo "docker $*" >> "$DSTATE/calls"
case "$1" in
  info) [ -z "${DOCKER_DOWN:-}" ] ;;
  image)
    case "$2" in
      inspect) grep -qxF "$3" "$DSTATE/images" ;;
      prune) exit "${DOCKER_PRUNE_RC:-0}" ;;
    esac ;;
  images) img="${*: -1}"; grep "^$img:" "$DSTATE/images" | sed "s#^$img:##" ;;
  rmi) grep -qxF "$2" "$DSTATE/inuse" && { echo "in use" >&2; exit 1; }
       grep -qxF "$2" "$DSTATE/images" || exit 1
       grep -vxF "$2" "$DSTATE/images" > "$DSTATE/i.tmp"; mv "$DSTATE/i.tmp" "$DSTATE/images" ;;
  build) t=""; while [ $# -gt 0 ]; do [ "$1" = -t ] && t="$2"; shift; done
         echo "$t" >> "$DSTATE/images"
         if [ -n "${DOCKER_BUILD_HOOK:-}" ]; then eval "$DOCKER_BUILD_HOOK"; fi ;;
  builder) exit "${DOCKER_PRUNE_RC:-0}" ;;
esac
EOF
cat > "$STUBS/cargo" <<'EOF'
#!/usr/bin/env bash
echo "cargo $* TAG=${UR_IMAGE_TAG:-} GIT_DIR=${GIT_DIR:-}" >> "$DSTATE/cargo-calls"
case "$1" in
  -V) echo "cargo 1.90.0 (stub)" ;;
  metadata) echo "{\"packages\":[],\"workspace_root\":\"$PWD\",\"version\":1}" ;;
  tree) [ -n "${CARGO_TREE_FAIL:-}" ] && exit 101
        [ -n "${CARGO_TREE_EMPTY:-}" ] && exit 0
        echo "ur-server v0.1.0 ($PWD/crates/server)"
        echo "tokio v1.0.0"
        echo "workerd v0.1.0 ($PWD/crates/workerd)" ;;
  zigbuild|build) [ -n "${CARGO_BUILD_FAIL:-}" ] && exit 101
        t=target/release; [ "$1" = zigbuild ] && t="target/$4/release"
        mkdir -p "$t"; for b in ur-server workerd ur-workertools; do echo "bin $b" > "$t/$b"; done ;;
  test) if [ -n "${CARGO_TEST_HOOK:-}" ]; then eval "$CARGO_TEST_HOOK"; fi; exit "${CARGO_TEST_RC:-0}" ;;
  make) echo "make $2" >> "$DSTATE/make-calls"
        case "$2" in ci) exit "${MAKE_CI_RC:-0}" ;; acceptance-ci) exit "${MAKE_ACC_RC:-0}" ;; esac ;;
  sweep) exit 0 ;;
esac
EOF
cat > "$STUBS/rustc" <<'EOF'
#!/usr/bin/env bash
echo "rustc ${RUSTC_VER:-1.90.0} (stub)"; echo "host: aarch64-apple-darwin"
EOF
# git / ln: pass through to the real tools unless a fault is injected.
cat > "$STUBS/git" <<EOF
#!/usr/bin/env bash
if [ "\${1:-}" = ls-files ]; then
  [ -n "\${GIT_LSFILES_EMPTY:-}" ] && exit 0
  if [ -n "\${GIT_LSFILES_FAIL:-}" ]; then
    case " \$* " in *" --exclude-standard "*) echo "fatal: injected ls-files failure" >&2; exit 128 ;; esac
  fi
fi
exec "$REAL_GIT" "\$@"
EOF
cat > "$STUBS/ln" <<EOF
#!/usr/bin/env bash
if [ -n "\${LN_FAIL_ONCE:-}" ] && [ ! -e "\$DSTATE/ln-failed" ]; then
  : > "\$DSTATE/ln-failed"; echo "ln: injected transient failure" >&2; exit 1
fi
exec "$REAL_LN" "\$@"
EOF
chmod +x "$STUBS"/*

# ------------------------------------------------------------ fake repo ----
G() { HOME="$SANDBOX_HOME" "$REAL_GIT" -c user.email=t@t -c user.name=t -c commit.gpgsign=false \
        -c protocol.file.allow=always "$@"; }

# One template repo, copied per test: crates (two in the image closure, one
# not), proto, docs, every container context, the scripts under test, and a
# file-URL submodule inside a container context whose own .gitignore ignores
# *.local (Docker still sends those files).
make_template() {
    local T="$S/template" SUB="$S/subsrc" f
    mkdir -p "$T" "$SUB"
    (cd "$SUB" && G init -q && echo s > s.txt && echo '*.local' > .gitignore \
        && G add . && G commit -qm s) || return 1
    cd "$T" || return 1
    G init -q
    mkdir -p crates/server/src crates/workerd/src crates/unused proto scripts/build ur-hooks/git \
        containers/server containers/worker-base containers/worker-claude containers/worker-codex \
        containers/worker-agy containers/squid docs
    echo '[workspace]' > Cargo.toml; echo lock > Cargo.lock
    echo 'fn main(){}' > crates/server/src/main.rs; echo 'fn main(){}' > crates/workerd/src/main.rs
    echo x > crates/unused/lib.rs; echo 'syntax="proto3";' > proto/a.proto; echo doc > docs/a.md
    # Names whose order differs between C and UTF-8 collation (t_hash_locale).
    echo a > containers/server/a; echo B > containers/server/B
    for f in server worker-base worker-claude worker-codex worker-agy squid; do
        echo "FROM x" > "containers/$f/Dockerfile"
    done
    cp "$SRC/scripts/acceptance-ci.sh" scripts/
    for f in agent-images ci-tags ci-lock image-hash stage image housekeeping; do
        cp "$SRC/scripts/build/$f.sh" scripts/build/
    done
    cp "$SRC/ur-hooks/git/pre-push" ur-hooks/git/
    cp "$SRC/.gitignore" .gitignore
    printf '%s\n' crates/server/ignored.txt containers/server/ignored.txt >> .gitignore
    G add . && G commit -qm init || return 1
    G submodule add -q "$SUB" containers/worker-base/vendor/sub >/dev/null 2>&1 || return 1
    G commit -qm sub || return 1
    cd - >/dev/null || return 1
}

# Per-test state: a fresh repo copy, docker store and UR_CONFIG.
R="$S/repo"
DSTATE="$S/dstate"
CFG="$S/urcfg"
LOCK_PATH="$CFG/locks/pre-push.lock"
reset_state() {
    chmod -R u+w "$R" "$CFG" 2>/dev/null
    rm -rf "$R" "$DSTATE" "$CFG"
    cp -R "$S/template" "$R" && mkdir -p "$DSTATE" "$CFG" && touch "$DSTATE/images" "$DSTATE/inuse"
}

sandbox_env() { env -i HOME="$SANDBOX_HOME" PATH="$BASEPATH" DSTATE="$DSTATE" "$@"; }
hash_() { (cd "$R" && sandbox_env "$@" bash scripts/build/image-hash.sh 2>"$S/hash.err"); }
acc() { (cd "$R" && "${TO[@]}" env -i HOME="$SANDBOX_HOME" PATH="$BASEPATH" DSTATE="$DSTATE" \
            UR_CONFIG="$CFG" UR_LOCK_POLL_SECS=1 "$@" bash scripts/acceptance-ci.sh >"$S/acc.out" 2>&1); }
acc_bg() { (cd "$R" && exec "${TO[@]}" env -i HOME="$SANDBOX_HOME" PATH="$BASEPATH" DSTATE="$DSTATE" \
            UR_CONFIG="$CFG" UR_LOCK_POLL_SECS=1 bash scripts/acceptance-ci.sh >"$S/acc.out" 2>&1) & }
hook() { (cd "$R" && echo "refs/heads/feat abc refs/heads/feat def" \
            | "${TO[@]}" env -i HOME="$SANDBOX_HOME" PATH="$BASEPATH" DSTATE="$DSTATE" GIT_DIR="$R/.git" \
              "$@" bash ur-hooks/git/pre-push >"$S/hook.out" 2>&1); }

IMGS="ur-worker-base ur-server ur-squid ur-worker-claude ur-worker-codex ur-worker-agy"
seed() { local i; for i in $IMGS; do echo "$i:ci-$1" >> "$DSTATE/images"; done; }
has_all() { local i; for i in $IMGS; do grep -qxF "$i:ci-$1" "$DSTATE/images" || return 1; done; }
has_none() { local i; for i in $IMGS; do grep -qxF "$i:ci-$1" "$DSTATE/images" && return 1; done; return 0; }
live_pid() { sleep 300 & BGPIDS+=($!); LIVE=$!; }
# Images for the pristine hash exist: acceptance-ci takes the warm path (no
# stage/build). For tests about the lock or pruning, not the build.
warm() { seed "$BASE"; }
dead_pid() { sleep 0 & DEAD=$!; wait "$DEAD"; }
# Wait (bounded) until $1 matches in file $2.
wait_for() { local i=0; until grep -q "$1" "$2" 2>/dev/null; do i=$((i + 1)); [ $i -gt 600 ] && return 1; sleep 0.1; done; }

# ------------------------------------------------------------- image-hash --
# BASE is the hash of the pristine fake repo, computed once in main.
changes() { local b; (cd "$R" && eval "$1"); b=$(hash_) || return 1; [ "$BASE" != "$b" ]; }
same()    { local b; (cd "$R" && eval "$1"); b=$(hash_) || return 1; [ "$BASE" = "$b" ]; }
SUBDIR=containers/worker-base/vendor/sub

t_hash_stable()        { [ "$(hash_)" = "$BASE" ] && [ ${#BASE} -eq 12 ]; }
t_hash_locale()        { [ "$(hash_ LANG=en_US.UTF-8 LC_ALL=en_US.UTF-8)" = "$(hash_ LC_ALL=C)" ]; }
t_hash_crate()         { changes 'echo y >> crates/server/src/main.rs'; }
t_hash_proto()         { changes 'echo y >> proto/a.proto'; }
t_hash_cargo_lock()    { changes 'echo y >> Cargo.lock'; }
t_hash_scripts()       { changes 'echo "#" >> scripts/build/image.sh'; }
t_hash_ctx()           { changes 'echo y >> containers/squid/Dockerfile'; }
t_hash_ctx_ignored()   { changes 'echo y > containers/server/ignored.txt'; }
t_hash_sub_edits()     { local a b; (cd "$R" && echo e1 > $SUBDIR/s.txt); a=$(hash_) || return 1
                         (cd "$R" && echo e2 > $SUBDIR/s.txt); b=$(hash_) || return 1; [ "$a" != "$b" ]; }
t_hash_sub_ignored()   { changes "echo y > $SUBDIR/x.local"; }
t_hash_sub_describe()  { same "G -C $SUBDIR tag v9"; }
t_hash_rustflags()     { [ "$BASE" != "$(hash_ RUSTFLAGS=-Copt-level=1)" ]; }
t_hash_rustc()         { [ "$BASE" != "$(hash_ RUSTC_VER=1.91.0)" ]; }
t_hash_native()        { [ "$BASE" != "$(hash_ UR_NATIVE_BUILD=1)" ]; }
t_hash_cargo_cfg()     { changes 'mkdir -p .cargo && echo "[build]" > .cargo/config.toml'; }
t_hash_cargo_make_env() { [ "$BASE" = "$(hash_ CARGO_MAKE_TASK=x CARGO=/x CARGO_HOME=/y)" ]; }
t_hash_staged_outputs() { same 'echo b > containers/server/ur-server; echo b > containers/server/builderd;
                                mkdir -p containers/worker-claude/bin; echo b > containers/worker-claude/bin/workerd'; }
t_hash_unrelated()     { same 'echo y >> docs/a.md; echo y >> crates/unused/lib.rs; echo y > crates/server/ignored.txt'; }
t_hash_tree_fail()     { ! hash_ CARGO_TREE_FAIL=1 >/dev/null; }
t_hash_tree_empty()    { ! hash_ CARGO_TREE_EMPTY=1 >/dev/null && grep -q "found no workspace crates" "$S/hash.err"; }
t_hash_ls_fail()       { local out; out=$(hash_ GIT_LSFILES_FAIL=1) && return 1; [ -z "$out" ]; }
t_hash_ls_empty()      { ! hash_ GIT_LSFILES_EMPTY=1 >/dev/null && grep -q "found no input files" "$S/hash.err"; }

# ----------------------------------------------------------- acceptance-ci --
t_acc_cold() {
    acc || return 1
    local H=$BASE; has_all "$H" || return 1
    grep -q "^cargo test -p acceptance --features acceptance TAG=ci-$H" "$DSTATE/cargo-calls" || return 1
    grep -q " $H$" "$CFG/ci-image-tags" && [ ! -L "$LOCK_PATH" ]
}
t_acc_warm() {
    acc || return 1; : > "$DSTATE/cargo-calls"; acc || return 1
    ! grep -q zigbuild "$DSTATE/cargo-calls" && grep -q "already built" "$S/acc.out"
}
t_acc_update_agent() { seed "$BASE"; acc UR_UPDATE_AGENT=claude && grep -q zigbuild "$DSTATE/cargo-calls"; }
t_acc_docker_down()  { acc DOCKER_DOWN=1 && return 1; grep -q "docker unreachable" "$S/acc.out" && [ ! -e "$CFG/locks" ]; }
t_acc_stage_fail()   { acc CARGO_BUILD_FAIL=1 && return 1; ! grep -q "^cargo test" "$DSTATE/cargo-calls" && ! grep -q ':ci-' "$DSTATE/images"; }
t_acc_midbuild_edit() {
    local H=$BASE
    acc DOCKER_BUILD_HOOK='echo z >> crates/server/src/main.rs' && return 1
    grep -q "sources changed during build" "$S/acc.out" && has_none "$H" && ! grep -q "^cargo test" "$DSTATE/cargo-calls"
}
t_acc_midbuild_inuse() { # one just-built image is held by a container: the others must still go
    local H=$BASE i; echo "ur-worker-base:ci-$H" > "$DSTATE/inuse"
    acc DOCKER_BUILD_HOOK='echo z >> crates/server/src/main.rs' && return 1
    for i in ur-server ur-squid ur-worker-claude ur-worker-codex ur-worker-agy; do
        grep -qxF "$i:ci-$H" "$DSTATE/images" && return 1
    done
    return 0
}
t_acc_rehash_fail() {
    local H=$BASE
    acc DOCKER_BUILD_HOOK='chmod -x scripts/build/image-hash.sh' && return 1
    has_none "$H" && ! grep -q "^cargo test" "$DSTATE/cargo-calls"
}
t_acc_record_before_build() {
    local H=$BASE
    acc DOCKER_BUILD_HOOK='grep -q " '"$H"'$" "$UR_CONFIG/ci-image-tags" && touch "$DSTATE/logged-early"' || return 1
    [ -e "$DSTATE/logged-early" ]
}
t_acc_hash_after_lock() { # sources edited while waiting on the lock: the tag names the edited sources
    local p H; live_pid; mkdir -p "$CFG/locks"; ln -s "$LIVE:other" "$LOCK_PATH"
    acc_bg; p=$!
    wait_for "waiting for lock" "$S/acc.out" || return 1
    echo edit >> "$R/crates/server/src/main.rs"; kill "$LIVE"
    wait "$p" || return 1
    H=$(hash_); grep -q "TAG=ci-$H" "$DSTATE/cargo-calls"
}

# -------------------------------------------------------------------- lock --
t_lock_live_holder_waits() {
    local p alive=0; live_pid; mkdir -p "$CFG/locks"; ln -s "$LIVE:other" "$LOCK_PATH"
    acc_bg; p=$!
    wait_for "waiting for lock (PID $LIVE, branch other)" "$S/acc.out" || { kill "$p"; return 1; }
    sleep 1.5; kill -0 "$p" 2>/dev/null && alive=1
    pkill -P "$p" 2>/dev/null; kill "$p" 2>/dev/null; wait "$p" 2>/dev/null
    [ $alive = 1 ] && ! grep -q "^cargo test" "$DSTATE/cargo-calls" 2>/dev/null \
        && [ "$(readlink "$LOCK_PATH")" = "$LIVE:other" ]
}
t_lock_stale_pid()  { warm; dead_pid; mkdir -p "$CFG/locks"; ln -s "$DEAD:gone" "$LOCK_PATH"; acc && [ ! -L "$LOCK_PATH" ]; }
t_lock_holds_pid()  { # the lock names this run (pid + branch) for the whole run
    warm; acc UR_PUSH_BRANCH=feat CARGO_TEST_HOOK='[ "$(readlink "$UR_CONFIG/locks/pre-push.lock")" = "$PPID:feat" ] && touch "$DSTATE/held"' \
        && [ -e "$DSTATE/held" ]
}
t_lock_release_not_owner() { # another run took the lock over mid-run: exiting must not delete its lock
    warm; live_pid
    acc OTHER="$LIVE" CARGO_TEST_HOOK='rm -f "$UR_CONFIG/locks/pre-push.lock"; ln -s "$OTHER:new" "$UR_CONFIG/locks/pre-push.lock"' \
        || return 1
    [ "$(readlink "$LOCK_PATH")" = "$LIVE:new" ]
}
t_lock_unwritable() {
    local r; mkdir -p "$CFG/locks"; chmod 555 "$CFG/locks"
    acc; r=$?; chmod 755 "$CFG/locks"
    [ $r -ne 0 ] && [ $r -ne 124 ] && grep -q "cannot create lock" "$S/acc.out"
}
t_lock_dir_in_way() { mkdir -p "$LOCK_PATH"; acc && return 1; grep -q "is a directory" "$S/acc.out"; }
t_lock_transient_ln_failure() { warm; acc LN_FAIL_ONCE=1 && [ -e "$DSTATE/ln-failed" ]; }
# Deterministic replay of the double stale-takeover race: W1 and W2 both read
# the same stale lock; W2 takes it over and acquires; then W1 acts on its stale
# read. W2's live lock must survive and W1 must not get it.
t_lock_race_double_takeover() {
    dead_pid; live_pid; local W2="$LIVE"
    (
        export UR_CONFIG="$CFG"; mkdir -p "$CFG/locks"
        # shellcheck source=scripts/build/ci-lock.sh
        source "$R/scripts/build/ci-lock.sh"
        ln -s "$DEAD:stale" "$LOCK"
        observed=$(readlink "$LOCK")          # W1 and W2 both read it as stale
        take_over_stale_lock "$observed"      # W2 wins the rename
        ln -sn "$W2:w2" "$LOCK"               # ... and acquires
        take_over_stale_lock "$observed"      # W1 acts on its stale read
        [ "$(readlink "$LOCK")" = "$W2:w2" ] || exit 1
        ! ln -sn "$$:w1" "$LOCK" 2>/dev/null  # W1's acquire attempt must fail
    ) || return 1
    local f
    for f in "$CFG/locks/"*stale*; do [ -e "$f" ] && return 1; done
    return 0
}

# ------------------------------------------------------------------- prune --
prune_setup() {
    local h; warm; printf '100 h1\n200 h2\n300 h3\n400 h4\n' > "$CFG/ci-image-tags"
    for h in h1 h2 h3 h4 other; do seed $h; done
}
t_prune_keepset() { # keep 3 in total (current + 2 newest logged); never touch unlogged tags
    local H=$BASE; prune_setup; acc || return 1
    has_none h1 && has_none h2 && has_all h3 && has_all h4 && has_all "$H" && has_all other || return 1
    [ "$(awk '{print $2}' "$CFG/ci-image-tags" | sort | tr '\n' ' ')" = "$(printf '%s\n' h3 h4 "$H" | sort | tr '\n' ' ')" ]
}
t_prune_inuse_retained() {
    prune_setup; echo "ur-squid:ci-h1" > "$DSTATE/inuse"; acc || return 1
    grep -qxF ur-squid:ci-h1 "$DSTATE/images" && ! grep -qxF ur-server:ci-h1 "$DSTATE/images" \
        && grep -q ' h1$' "$CFG/ci-image-tags"
}
t_prune_biglog()        { local i; warm; for i in $(seq 1 5000); do echo "$i h$i"; done > "$CFG/ci-image-tags"
                          acc && [ "$(wc -l < "$CFG/ci-image-tags")" -le 3 ]; }
t_prune_log_unwritable() { warm; mkdir -p "$CFG/ci-image-tags"; acc; }
t_prune_log_unreadable() { prune_setup; chmod 000 "$CFG/ci-image-tags"; acc; local r=$?; chmod 644 "$CFG/ci-image-tags"
                           [ $r -eq 0 ] && has_all h1; }

# ------------------------------------------------- stage / housekeeping --
t_stage() {
    local c
    (cd "$R" && sandbox_env bash scripts/build/stage.sh >/dev/null 2>&1) || return 1
    for c in worker-claude worker-codex worker-agy; do
        [ "$(cat "$R/containers/$c/bin/workerd")" = "bin workerd" ] || return 1
        [ "$(cat "$R/containers/$c/bin/ur-workertools")" = "bin ur-workertools" ] || return 1
    done
    [ "$(cat "$R/containers/server/ur-server")" = "bin ur-server" ]
}
t_housekeeping_warns_never_fails() {
    (cd "$R" && sandbox_env DOCKER_PRUNE_RC=1 bash scripts/build/housekeeping.sh >/dev/null 2>"$S/hk.err") \
        && grep -q "docker builder prune failed" "$S/hk.err"
}

# ---------------------------------------------------------------- pre-push --
t_hook_ok()       { hook || return 1; [ -z "$(ls "$R/tmp" 2>/dev/null)" ] && grep -q "make ci" "$DSTATE/make-calls" \
                        && grep -q "make acceptance-ci" "$DSTATE/make-calls"; }
t_hook_ci_fail()  { hook MAKE_CI_RC=1 && return 1; ! grep -q "make acceptance-ci" "$DSTATE/make-calls" && [ -n "$(ls "$R/tmp")" ]; }
t_hook_acc_fail() { ! hook MAKE_ACC_RC=1; }
t_hook_housekeeping_runs() { hook MAKE_ACC_RC=1; grep -q "builder prune" "$DSTATE/calls"; }
t_hook_housekeeping_fails() { printf '#!/usr/bin/env bash\nexit 7\n' > "$R/scripts/build/housekeeping.sh"
                              hook || return 1; ! hook MAKE_CI_RC=1; }
t_hook_no_housekeeping() { rm "$R/scripts/build/housekeeping.sh"; hook && ! grep -q housekeeping "$S/hook.out"; }
t_hook_gitdir_cleared() { hook && ! grep -q "GIT_DIR=/" "$DSTATE/cargo-calls"; }

# -------------------------------------------------------------------- main --
if command -v perl >/dev/null 2>&1; then
    now() { perl -MTime::HiRes=time -e 'printf "%.2f", time'; }
    elapsed() { perl -e 'printf "%.1f", $ARGV[1] - $ARGV[0]' "$1" "$(now)"; }
else
    now() { date +%s; }
    elapsed() { echo $(( $(now) - $1 )); }
fi
make_template >"$S/template.log" 2>&1 || { cat "$S/template.log" >&2; echo "test-ci-scripts: could not build the fake repo" >&2; exit 1; }
if ! { reset_state && BASE=$(hash_) && [ -n "$BASE" ]; }; then
    cat "$S/hash.err" >&2
    echo "test-ci-scripts: image-hash.sh fails on the pristine fake repo" >&2
    exit 1
fi
FAILS=()
COUNT=0
for t in $(declare -F | awk '{print $3}' | grep -E '^t_'); do
    echo "$t" | grep -Eq "$ONLY" || continue
    COUNT=$((COUNT + 1))
    reset_state || { echo "FAIL $t (setup)"; FAILS+=("$t"); continue; }
    # Each test runs in a subshell; it kills the background processes it started.
    start=$(now)
    if ( trap kill_bg EXIT; "$t" ) >/dev/null 2>&1; then r="ok  "; else r=FAIL; FAILS+=("$t"); fi
    echo "$r $t ($(elapsed "$start")s)"
    [ -n "${UR_CI_TEST_FAIL_FAST:-}" ] && [ ${#FAILS[@]} -gt 0 ] && break
done
echo "test-ci-scripts: $((COUNT - ${#FAILS[@]}))/$COUNT passed"
[ ${#FAILS[@]} -eq 0 ]
