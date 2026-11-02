#!/usr/bin/env bash
# Tests for the CI shell layer: image-hash.sh, acceptance-ci.sh (+ ci-tags.sh,
# ci-lock.sh), stage.sh, housekeeping.sh and the pre-push hook.
#
#   scripts/build/test-ci-scripts.sh [--if-changed] [TEST_NAME_REGEX]
#
# --if-changed (how `cargo make ci` runs it): skip, with a one-line notice,
# unless scripts/acceptance-ci.sh, scripts/build/, ur-hooks/git/ or Makefile.toml
# differ from the merge base with origin/master (committed, uncommitted or
# untracked). Runs when origin/master is unknown; UR_CI_TEST_FORCE=1 forces it.
#
# Every test runs the real scripts in a throwaway git repo under a mktemp dir,
# with docker, cargo and rustc stubbed, HOME, UR_CONFIG and UR_CI_STATE_DIR
# pointed into the sandbox, and git fault injection available via a pass-through wrapper. It
# never touches real docker, images, ~/.ur or the real acceptance lock.
#
# UR_CI_TEST_SRC=<dir> tests the scripts under <dir> instead of this checkout
# (used to run mutants against the suite). Needs bash 3.2+, git, perl (the lock
# under test uses it; also the timeout fallback and timings) and a sha256 tool;
# uses `timeout` when present. Runs serially; each script invocation gets a generous
# timeout (UR_CI_TEST_STEP_TIMEOUT, default 120s) so a loaded machine slows the
# suite rather than flaking it. UR_CI_TEST_KEEP=1 keeps the sandbox for debugging;
# UR_CI_TEST_FAIL_FAST=1 stops at the first failing test (mutation runs).

# Test hooks (DOCKER_BUILD_HOOK, CARGO_TEST_HOOK) are single-quoted on purpose:
# the stubs eval them inside the script under test.
# shellcheck disable=SC2016
set -u

SELF_ROOT=$(cd "$(dirname "$0")/../.." && pwd -P)
SRC="${UR_CI_TEST_SRC:-$SELF_ROOT}"
STEP_TIMEOUT="${UR_CI_TEST_STEP_TIMEOUT:-120}"

# Hooks export repo-local git variables (GIT_DIR, ...); they must not leak into
# the sandbox repos.
# shellcheck disable=SC2046
unset $(git rev-parse --local-env-vars 2>/dev/null)

CI_PATHS='^(scripts/acceptance-ci\.sh|scripts/build/|ur-hooks/git/|Makefile\.toml$)'
if [ "${1:-}" = --if-changed ]; then
    shift
    if [ -z "${UR_CI_TEST_FORCE:-}" ] \
        && base=$(git -C "$SELF_ROOT" merge-base HEAD origin/master 2>/dev/null) \
        && changed=$(git -C "$SELF_ROOT" diff --name-only "$base" \
                     && git -C "$SELF_ROOT" ls-files -o --exclude-standard) \
        && ! grep -qE "$CI_PATHS" <<< "$changed"; then
        echo "test-ci-scripts: skipped (no CI script changes since origin/master; UR_CI_TEST_FORCE=1 runs it)"
        exit 0
    fi
fi
ONLY="${1:-.}"

if [ -x /usr/bin/git ]; then REAL_GIT=/usr/bin/git; else REAL_GIT=$(command -v git); fi
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
  metadata) if [ -n "${CARGO_LOCK_STALE:-}" ] && [[ " $* " == *" --locked "* ]]; then
              echo "error: the lock file needs to be updated but --locked was passed" >&2; exit 101
            fi
            echo "{\"packages\":[],\"workspace_root\":\"$PWD\",\"version\":1}" ;;
  tree) # Like the real one: prints the closure of each -p only, and refuses
        # edge kinds / targets other than the ones image-hash.sh must pass.
        [ -n "${CARGO_TREE_FAIL:-}" ] && exit 101
        [ -n "${CARGO_TREE_EMPTY:-}" ] && exit 0
        shift; pkgs=""; edges=""; target=""
        while [ $# -gt 0 ]; do
          case "$1" in
            -p) pkgs="$pkgs $2"; shift ;;
            -e) edges="$2"; shift ;;
            --target) target="$2"; shift ;;
            --format|--prefix) shift ;;
            --quiet|--locked) ;;
            *) echo "stub cargo tree: unexpected argument $1" >&2; exit 2 ;;
          esac
          shift
        done
        [ "$edges" = normal,build ] || { echo "stub cargo tree: unexpected -e '$edges'" >&2; exit 2; }
        [ "$target" = all ] || { echo "stub cargo tree: unexpected --target '$target'" >&2; exit 2; }
        for p in $pkgs; do
          case "$p" in
            ur-server) echo "ur-server v0.1.0 ($PWD/crates/server)"; echo "ur_utils v0.1.0 ($PWD/crates/ur_utils)"
                       echo "tokio v1.0.0" ;;
            workerd) echo "workerd v0.1.0 ($PWD/crates/workerd)"; echo "tmux v0.1.0 ($PWD/crates/tmux)" ;;
            ur-workertools) echo "ur-workertools v0.1.0 ($PWD/crates/ur-workertools)"
                            echo "ur_utils v0.1.0 ($PWD/crates/ur_utils)" ;;
            *) echo "stub cargo tree: unknown package $p" >&2; exit 101 ;;
          esac
        done ;;
  zigbuild|build) # Builds only the -p packages, into target/[<target>/]release;
        # each binary records which subcommand built it ("bin <name> via <cmd>").
        [ -n "${CARGO_BUILD_FAIL:-}" ] && exit 101
        if [ -n "${CARGO_BUILD_HOOK:-}" ]; then eval "$CARGO_BUILD_HOOK"; fi
        cmd="$1"; shift; pkgs=""; t=target/release
        while [ $# -gt 0 ]; do
          case "$1" in
            -p) pkgs="$pkgs $2"; shift ;;
            --target) t="target/$2/release"; shift ;;
          esac
          shift
        done
        mkdir -p "$t"; for b in $pkgs; do echo "bin $b via $cmd" > "$t/$b"; done ;;
  test) if [ -n "${CARGO_TEST_HOOK:-}" ]; then eval "$CARGO_TEST_HOOK"; fi; exit "${CARGO_TEST_RC:-0}" ;;
  make) echo "make $2" >> "$DSTATE/make-calls"
        case "$2" in ci) exit "${MAKE_CI_RC:-0}" ;; acceptance-ci) exit "${MAKE_ACC_RC:-0}" ;; esac ;;
  sweep) exit 0 ;;
esac
EOF
# cargo-sweep: present so housekeeping's `command -v cargo-sweep` finds it; the
# sweep itself goes through `cargo sweep`, logged by the cargo stub.
printf '#!/usr/bin/env bash\nexit 0\n' > "$STUBS/cargo-sweep"
cat > "$STUBS/cargo-zigbuild" <<'EOF'
#!/usr/bin/env bash
[ "${1:-}" = --version ] || { echo "stub cargo-zigbuild: only --version is supported" >&2; exit 2; }
echo "cargo-zigbuild ${ZIGBUILD_VER:-0.22.2}"
EOF
cat > "$STUBS/rustc" <<'EOF'
#!/usr/bin/env bash
echo "rustc ${RUSTC_VER:-1.90.0} (stub)"; echo "host: aarch64-apple-darwin"
EOF
# git: pass through to the real git unless a fault is injected.
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
chmod +x "$STUBS"/*

# ------------------------------------------------------------ fake repo ----
# No global or system git config may reach the sandbox (signing, excludesFile,
# hooksPath, ...): every git and script invocation gets GIT_ENV.
GIT_ENV=(GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1)
G() { env HOME="$SANDBOX_HOME" "${GIT_ENV[@]}" "$REAL_GIT" -c user.email=t@t -c user.name=t -c commit.gpgsign=false \
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
    mkdir -p crates/server/src crates/workerd/src crates/ur-workertools/src crates/ur_utils crates/tmux \
        crates/unused proto scripts/build ur-hooks/git \
        containers/server containers/worker-base containers/worker-claude containers/worker-codex \
        containers/worker-agy containers/squid docs
    echo '[workspace]' > Cargo.toml; echo lock > Cargo.lock
    echo 'fn main(){}' > crates/server/src/main.rs; echo 'fn main(){}' > crates/workerd/src/main.rs
    echo 'fn main(){}' > crates/ur-workertools/src/main.rs; echo u > crates/ur_utils/lib.rs
    echo t > crates/tmux/lib.rs
    echo x > crates/unused/lib.rs; echo 'syntax="proto3";' > proto/a.proto; echo doc > docs/a.md
    # Names whose order differs between C and UTF-8 collation (t_hash_locale).
    echo a > containers/server/a; echo B > containers/server/B
    for f in server worker-base worker-claude worker-codex worker-agy squid; do
        echo "FROM x" > "containers/$f/Dockerfile"
    done
    cp "$SRC/scripts/acceptance-ci.sh" scripts/
    for f in agent-images ci-tags ci-lock image-hash stage image housekeeping install-hooks test-ci-scripts; do
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
LOCKS="$CFG/locks"              # legacy directory lock lives here (old scripts)
STATE="$S/cistate"              # UR_CI_STATE_DIR: flock + tag log
FLOCKS="$STATE/locks"
TAGLOG="$STATE/ci-image-tags"
# Never let a script under test resolve a state path to the real ~/.ur.
for p in "$SANDBOX_HOME" "$CFG" "$STATE"; do
    case "$p" in
        "$S"/*) ;;
        *) echo "test-ci-scripts: $p is outside the sandbox" >&2; exit 1 ;;
    esac
done
case "$S" in "$HOME"/.ur|"$HOME"/.ur/*) echo "test-ci-scripts: sandbox is inside ~/.ur" >&2; exit 1 ;; esac
reset_state() {
    chmod -R u+w "$R" "$CFG" "$STATE" 2>/dev/null
    rm -rf "$R" "$S/repo2" "$S/ch" "$S/wt" "$DSTATE" "$CFG" "$S/urcfgB" "$STATE" "$SANDBOX_HOME"
    cp -R "$S/template" "$R" && mkdir -p "$DSTATE" "$CFG" "$STATE" "$SANDBOX_HOME" \
        && touch "$DSTATE/images" "$DSTATE/inuse"
}

SANDBOX_VARS=(HOME="$SANDBOX_HOME" PATH="$BASEPATH" DSTATE="$DSTATE" UR_CONFIG="$CFG" UR_CI_STATE_DIR="$STATE")
sandbox_env() { env -i "${SANDBOX_VARS[@]}" "${GIT_ENV[@]}" "$@"; }
hash_() { (cd "$R" && sandbox_env "$@" bash scripts/build/image-hash.sh 2>"$S/hash.err"); }
hash_in() { (cd "$1" && sandbox_env bash scripts/build/image-hash.sh 2>"$S/hash.err"); }
# Foreground run in $ACC_REPO (default $R); extra env assignments as arguments.
acc() { (cd "${ACC_REPO:-$R}" && "${TO[@]}" env -i "${SANDBOX_VARS[@]}" "${GIT_ENV[@]}" \
            UR_LOCK_POLL_SECS=1 "$@" bash scripts/acceptance-ci.sh >"$S/acc.out" 2>&1); }
# Background run with extra env assignments as arguments; output to $ACC_OUT
# (default $S/acc.out). $! is the run.
acc_bg() { (cd "$R" && exec "${TO[@]}" env -i "${SANDBOX_VARS[@]}" "${GIT_ENV[@]}" \
            UR_LOCK_POLL_SECS=1 "$@" bash scripts/acceptance-ci.sh >"${ACC_OUT:-$S/acc.out}" 2>&1) & }
hook() { (cd "$R" && echo "refs/heads/feat abc refs/heads/feat def" \
            | "${TO[@]}" env -i "${SANDBOX_VARS[@]}" "${GIT_ENV[@]}" GIT_DIR="$R/.git" \
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
# A locale whose collation puts "a" before "B" (C/byte order puts B first);
# C.UTF-8 collates like C, so it does not count. UR_CI_TEST_COLLATE_LOCALE
# overrides the candidates.
collating_locale() {
    local l
    for l in ${UR_CI_TEST_COLLATE_LOCALE:-en_US.UTF-8 $(locale -a 2>/dev/null | grep -iE '\.utf-?8$' | grep -vE '^(C|POSIX)\.')}; do
        if [ "$(printf 'a\nB\n' | LC_ALL="$l" sort 2>/dev/null | head -n 1)" = a ]; then
            echo "$l"; return 0
        fi
    done
    return 1
}
t_hash_locale() {
    local l; l=$(collating_locale) || return 77
    [ "$(hash_ LANG="$l" LC_ALL="$l")" = "$(hash_ LC_ALL=C)" ]
}
t_hash_crate()         { changes 'echo y >> crates/server/src/main.rs'; }
t_hash_workerd()       { changes 'echo y >> crates/workerd/src/main.rs'; }
t_hash_workertools()   { changes 'echo y >> crates/ur-workertools/src/main.rs'; }
t_hash_dep_crate()     { changes 'echo y >> crates/tmux/lib.rs'; }
t_hash_git_hermetic()  { # host git config must not leak in: a global excludesFile
    local a b                # ignoring everything must not hide an untracked crate file
    echo y > "$R/crates/server/src/new.rs"; a=$(hash_) || return 1
    echo '*' > "$SANDBOX_HOME/ignore-all"
    printf '[core]\n\texcludesFile = %s\n' "$SANDBOX_HOME/ignore-all" > "$SANDBOX_HOME/.gitconfig"
    b=$(hash_) || return 1
    [ "$a" = "$b" ] && [ "$a" != "$BASE" ]
}
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
t_hash_target_dir_env() { # where the output goes / how many jobs: not an input
    [ "$BASE" = "$(hash_ CARGO_TARGET_DIR=/elsewhere CARGO_BUILD_TARGET_DIR=/x CARGO_BUILD_JOBS=3)" ]; }
t_hash_encoded_rustflags() { [ "$BASE" != "$(hash_ CARGO_ENCODED_RUSTFLAGS=-Copt-level=1)" ]; }
t_hash_deleted_tracked() { changes 'rm containers/squid/Dockerfile'; }
t_hash_uninit_submodule() { # an uninitialized submodule still hashes, stably
    local a b; (cd "$R" && G submodule --quiet deinit -f "$SUBDIR") || return 1
    [ -z "$(ls -A "$R/$SUBDIR")" ] || return 1
    a=$(hash_) && b=$(hash_) && [ -n "$a" ] && [ "$a" = "$b" ]
}
t_hash_other_path() { cp -R "$R" "$S/repo2" && [ "$(hash_in "$S/repo2")" = "$BASE" ]; }
t_hash_user_cargo_config() { # ~/.cargo/config.toml (or $CARGO_HOME's, or the legacy name) is an input
    changes 'mkdir -p "$SANDBOX_HOME/.cargo" && echo "[build]" > "$SANDBOX_HOME/.cargo/config.toml"' || return 1
    mkdir -p "$S/ch" && echo '[profile.release]' > "$S/ch/config" || return 1
    [ "$BASE" != "$(hash_ CARGO_HOME="$S/ch")" ] && [ "$BASE" = "$(hash_ CARGO_HOME="$S/empty-cargo-home")" ]
}
t_hash_zigbuild_version() { [ "$BASE" != "$(hash_ ZIGBUILD_VER=9.9.9)" ]; }
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
    grep -q " $H$" "$TAGLOG" && [ ! -e "$LOCKS/pre-push" ]
}
t_acc_warm() {
    acc || return 1; : > "$DSTATE/cargo-calls"; acc || return 1
    ! grep -q "^cargo zigbuild --release" "$DSTATE/cargo-calls" && grep -q "already built" "$S/acc.out"
}
t_acc_update_agent() { seed "$BASE"; acc UR_UPDATE_AGENT=claude && grep -q "^cargo zigbuild --release" "$DSTATE/cargo-calls"; }
t_acc_force_base()   { warm; acc UR_FORCE_REBUILD_BASE=1 && grep -q "^cargo zigbuild --release" "$DSTATE/cargo-calls"; }
t_acc_docker_down()  { acc DOCKER_DOWN=1 && return 1; grep -q "docker unreachable" "$S/acc.out" && [ ! -e "$FLOCKS" ]; }
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
    acc DOCKER_BUILD_HOOK='grep -q " '"$H"'$" "$UR_CI_STATE_DIR/ci-image-tags" && touch "$DSTATE/logged-early"' || return 1
    [ -e "$DSTATE/logged-early" ]
}
t_acc_hash_after_lock() { # sources edited while waiting on the lock: the tag names the edited sources
    local p H; hold_lock || return 1
    acc_bg; p=$!
    wait_for "waiting for lock" "$S/acc.out" || return 1
    echo edit >> "$R/crates/server/src/main.rs"; kill -9 "$HOLDER"
    wait "$p" || return 1
    H=$(hash_); grep -q "TAG=ci-$H" "$DSTATE/cargo-calls"
}

# -------------------------------------------------------------------- lock --
wait_file() { local i=0; until [ -e "$1" ]; do i=$((i + 1)); [ $i -gt 600 ] && return 1; sleep 0.1; done; }
# A background process holding the flock the way acceptance-ci does (fd 9 +
# perl), with branch "other". Sets HOLDER.
hold_lock() {
    mkdir -p "$FLOCKS"
    bash -c 'exec 9>>"$1/pre-push.flock"
             perl -MFcntl=:flock -e "open(my \$f, q(>&=), 9) or die; flock(\$f, LOCK_EX) or die" || exit 1
             echo "$$:other" > "$1/pre-push.holder"; : > "$1/test-held"
             while :; do sleep 0.2 9>&-; done' _ "$FLOCKS" &
    HOLDER=$!; BGPIDS+=("$HOLDER")
    wait_file "$FLOCKS/test-held"
}
# The lock code of master's acceptance-ci.sh before ur-1pijn (18374fa), verbatim
# except `sleep 10` -> `sleep 1`: a directory lock with a pid file, taken over
# when `kill -0 <pid>` fails. Runs in the background until killed; touches
# $UR_CONFIG/old-acquired once it holds the lock. Sets OLD.
cat > "$S/old-lock.sh" <<'EOF'
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
        sleep 1
    done
}

mkdir -p "$(dirname "$LOCK_DIR")"
acquire_lock
: > "$UR_CONFIG/old-acquired"
while :; do sleep 0.2; done
EOF
old_script_bg() { env -i "${SANDBOX_VARS[@]}" bash "$S/old-lock.sh" >"$S/old.out" 2>&1 & OLD=$!; BGPIDS+=("$OLD"); }

t_lock_second_holder_waits() { # waits while held, naming the holder; proceeds once it is gone
    local p; warm; hold_lock || return 1
    acc_bg; p=$!
    wait_for "waiting for lock (PID $HOLDER, branch other)" "$S/acc.out" || return 1
    sleep 1.5; kill -0 "$p" 2>/dev/null || return 1
    grep -q "^cargo test" "$DSTATE/cargo-calls" 2>/dev/null && return 1
    kill -9 "$HOLDER"; wait "$p" && grep -q "^cargo test" "$DSTATE/cargo-calls"
}
t_lock_sigkill_released() { # a SIGKILLed holder leaves nothing that blocks the next run
    local p; warm
    acc_bg CARGO_TEST_HOOK='echo $PPID > "$DSTATE/acc-pid"; while [ ! -e "$DSTATE/go" ]; do sleep 0.1; done'; p=$!
    wait_file "$DSTATE/acc-pid" || return 1
    kill -9 "$(cat "$DSTATE/acc-pid")"; : > "$DSTATE/go"; wait "$p" 2>/dev/null
    acc || return 1
    ! grep -q "waiting for lock" "$S/acc.out"
}
t_lock_child_does_not_hold() { # a process left behind by the run must not keep the lock
    warm
    acc CARGO_TEST_HOOK='sleep 30 & echo $! > "$DSTATE/child"' || return 1
    BGPIDS+=("$(cat "$DSTATE/child")")
    acc || return 1
    ! grep -q "waiting for lock" "$S/acc.out"
}
t_lock_build_children_do_not_hold() { # same for processes left behind by stage.sh / image.sh
    acc CARGO_BUILD_HOOK='sleep 30 & echo $! >> "$DSTATE/child"' \
        DOCKER_BUILD_HOOK='[ -e "$DSTATE/dchild" ] || { sleep 30 & echo $! > "$DSTATE/dchild"; }' || return 1
    local c; while read -r c; do BGPIDS+=("$c"); done < <(cat "$DSTATE/child" "$DSTATE/dchild")
    acc || return 1
    ! grep -q "waiting for lock" "$S/acc.out"
}
t_lock_cross_cfg_serialize() { # different UR_CONFIGs share the docker daemon: still one at a time
    local p1 p2 hook='[ -e "$DSTATE/inside" ] && touch "$DSTATE/overlap"; touch "$DSTATE/inside"; sleep 1; rm -f "$DSTATE/inside"'
    warm; mkdir -p "$S/urcfgB"
    ACC_OUT="$S/acc1.out" acc_bg CARGO_TEST_HOOK="$hook"; p1=$!
    ACC_OUT="$S/acc2.out" acc_bg UR_CONFIG="$S/urcfgB" CARGO_TEST_HOOK="$hook"; p2=$!
    wait "$p1" && wait "$p2" || return 1
    [ "$(grep -c "^cargo test" "$DSTATE/cargo-calls")" = 2 ] && [ ! -e "$DSTATE/overlap" ]
}
t_lock_concurrent_runs_serialize() {
    local p1 p2 hook='[ -e "$DSTATE/inside" ] && touch "$DSTATE/overlap"; touch "$DSTATE/inside"; sleep 1; rm -f "$DSTATE/inside"'
    warm
    ACC_OUT="$S/acc1.out" acc_bg CARGO_TEST_HOOK="$hook"; p1=$!
    ACC_OUT="$S/acc2.out" acc_bg CARGO_TEST_HOOK="$hook"; p2=$!
    wait "$p1" && wait "$p2" || return 1
    [ "$(grep -c "^cargo test" "$DSTATE/cargo-calls")" = 2 ] && [ ! -e "$DSTATE/overlap" ]
}
t_lock_holds_during_run() { # holder file and legacy dir name this run while it runs, and go away after
    warm
    acc UR_PUSH_BRANCH=feat CARGO_TEST_HOOK='[ "$(cat "$UR_CI_STATE_DIR/locks/pre-push.holder")" = "$PPID:feat" ] &&
        [ "$(cat "$UR_CONFIG/locks/pre-push/pid")" = "$PPID" ] && [ -e "$UR_CONFIG/locks/pre-push/flock-owner" ] &&
        touch "$DSTATE/held"' || return 1
    [ -e "$DSTATE/held" ] && [ ! -e "$LOCKS/pre-push" ] && [ ! -e "$FLOCKS/pre-push.holder" ]
}
t_lock_unwritable() { # fails fast with a message instead of spinning
    local r; mkdir -p "$FLOCKS"; chmod 555 "$FLOCKS"
    acc; r=$?; chmod 755 "$FLOCKS"
    [ $r -ne 0 ] && [ $r -ne 124 ] && grep -q "cannot open lock file" "$S/acc.out"
}
# Legacy directory lock (old master scripts).
t_lock_legacy_old_holds_new_waits() { # old script holds: new waits; old killed -> new takes it over
    local p; warm; old_script_bg; wait_file "$CFG/old-acquired" || return 1
    acc_bg; p=$!
    wait_for "older acceptance-ci (PID $OLD)" "$S/acc.out" || return 1
    grep -q "^cargo test" "$DSTATE/cargo-calls" 2>/dev/null && return 1
    kill -9 "$OLD"; wait "$p" && [ ! -e "$LOCKS/pre-push" ]
}
t_lock_legacy_new_holds_old_waits() { # new holds: the old script waits, then gets it after release
    local p; warm
    acc_bg CARGO_TEST_HOOK=': > "$DSTATE/in-test"; while [ ! -e "$DSTATE/go" ]; do sleep 0.1; done'; p=$!
    wait_file "$DSTATE/in-test" || return 1
    old_script_bg; wait_for "waiting for lock" "$S/old.out" || return 1
    sleep 1.5; [ -e "$CFG/old-acquired" ] && return 1
    : > "$DSTATE/go"; wait "$p" || return 1
    wait_file "$CFG/old-acquired"
}
t_lock_legacy_stale() { dead_pid; warm; mkdir -p "$LOCKS/pre-push"; echo "$DEAD" > "$LOCKS/pre-push/pid"
                        acc && [ ! -e "$LOCKS/pre-push" ]; }
t_lock_legacy_marked_reused_pid() { # our own leftover (marked) dir whose pid now belongs to someone else
    local p; warm; live_pid; mkdir -p "$LOCKS/pre-push"; : > "$LOCKS/pre-push/flock-owner"
    echo "$LIVE" > "$LOCKS/pre-push/pid"
    acc || return 1
    ! grep -q "older acceptance-ci" "$S/acc.out" && [ ! -e "$LOCKS/pre-push" ]
}
t_lock_legacy_pidless() { warm; mkdir -p "$LOCKS/pre-push"; acc && [ ! -e "$LOCKS/pre-push" ]; }
t_lock_legacy_release_not_owner() { # never remove a legacy dir that another pid owns by now
    warm; live_pid
    acc OTHER="$LIVE" CARGO_TEST_HOOK='echo "$OTHER" > "$UR_CONFIG/locks/pre-push/pid"' || return 1
    [ "$(cat "$LOCKS/pre-push/pid" 2>/dev/null)" = "$LIVE" ]
}

# ------------------------------------------------------------------- prune --
prune_setup() {
    local h; warm; printf '100 h1\n200 h2\n300 h3\n400 h4\n' > "$TAGLOG"
    for h in h1 h2 h3 h4 other; do seed $h; done
}
t_prune_keepset() { # keep 3 in total (current + 2 newest logged); never touch unlogged tags
    local H=$BASE; prune_setup; acc || return 1
    has_none h1 && has_none h2 && has_all h3 && has_all h4 && has_all "$H" && has_all other || return 1
    [ "$(awk '{print $2}' "$TAGLOG" | sort | tr '\n' ' ')" = "$(printf '%s\n' h3 h4 "$H" | sort | tr '\n' ' ')" ]
}
t_prune_inuse_retained() {
    prune_setup; echo "ur-squid:ci-h1" > "$DSTATE/inuse"; acc || return 1
    grep -qxF ur-squid:ci-h1 "$DSTATE/images" && ! grep -qxF ur-server:ci-h1 "$DSTATE/images" \
        && grep -q ' h1$' "$TAGLOG"
}
t_prune_cross_cfg_shared_hash() {
    # Run B (another UR_CONFIG, another checkout) just used hash HB, which A's
    # earlier use logged as old. A's prune must see B's use and keep HB.
    local HB; cp -R "$R" "$S/repo2" && echo b >> "$S/repo2/crates/server/src/main.rs" || return 1
    HB=$(hash_in "$S/repo2") || return 1
    printf '1 %s\n2 h2\n3 h3\n' "$HB" > "$TAGLOG"; cp "$TAGLOG" "$CFG/ci-image-tags"  # old (per-UR_CONFIG) location too
    seed "$HB"; seed h2; seed h3; warm; mkdir -p "$S/urcfgB"
    ACC_REPO="$S/repo2" acc UR_CONFIG="$S/urcfgB" || return 1
    acc || return 1
    has_all "$HB" && has_none h2
}
t_prune_biglog()        { local i; warm; for i in $(seq 1 5000); do echo "$i h$i"; done > "$TAGLOG"
                          acc && [ "$(wc -l < "$TAGLOG")" -le 3 ]; }
t_prune_log_unwritable() { warm; mkdir -p "$TAGLOG"; acc; }
t_prune_log_unreadable() { prune_setup; chmod 000 "$TAGLOG"; acc; local r=$?; chmod 644 "$TAGLOG"
                           [ $r -eq 0 ] && has_all h1; }

# ------------------------------------------------- stage / housekeeping --
t_install_hooks_from_worktree() { # .git is a file in a linked worktree
    local wt="$S/wt" h
    (cd "$R" && G worktree add -q "$wt" >/dev/null 2>&1) || return 1
    (cd "$wt" && sandbox_env bash scripts/build/install-hooks.sh >/dev/null 2>&1) || return 1
    h=$(cd "$R" && G rev-parse --git-path hooks)
    [ -f "$R/$h/pre-push" ] || [ -f "$h/pre-push" ]
}
t_acc_from_other_cwd() { warm; (cd "$R/crates" && "${TO[@]}" env -i "${SANDBOX_VARS[@]}" "${GIT_ENV[@]}" \
        bash ../scripts/acceptance-ci.sh >"$S/acc.out" 2>&1) && grep -q "^cargo test" "$DSTATE/cargo-calls"; }
# --if-changed, run from the fake repo against a fake origin/master. The inner
# run selects no tests (^t_none$), so "0/0 passed" means it ran.
if_changed() { (cd "$R" && sandbox_env "$@" bash scripts/build/test-ci-scripts.sh --if-changed '^t_none$' 2>&1); }
t_if_changed() {
    (cd "$R" && G update-ref refs/remotes/origin/master HEAD) || return 1
    if_changed | grep -q "skipped (no CI script changes" || return 1
    echo y >> "$R/docs/a.md"
    if_changed | grep -q "skipped" || return 1
    if_changed UR_CI_TEST_FORCE=1 | grep -q "0/0 passed" || return 1
    echo "# x" >> "$R/scripts/build/image.sh"
    if_changed | grep -q "0/0 passed" || return 1
    (cd "$R" && G checkout -q scripts/build/image.sh) && echo new > "$R/scripts/build/new.sh"
    if_changed | grep -q "0/0 passed" || return 1
    rm "$R/scripts/build/new.sh"; (cd "$R" && G update-ref -d refs/remotes/origin/master)
    if_changed | grep -q "0/0 passed"
}
t_stage_once() { # one cross-compile for all three binaries
    (cd "$R" && sandbox_env bash scripts/build/stage.sh >/dev/null 2>&1) || return 1
    [ "$(grep -c '^cargo zigbuild --release' "$DSTATE/cargo-calls")" = 1 ] \
        && grep -q -- '-p ur-server -p workerd -p ur-workertools' "$DSTATE/cargo-calls"
}
# stage <mode-env...>: run stage.sh; then check each staged binary was built by
# the expected subcommand ($SERVER_VIA for ur-server, $WORKER_VIA for workers).
staged_as() {
    local c
    for c in worker-claude worker-codex worker-agy; do
        [ "$(cat "$R/containers/$c/bin/workerd")" = "bin workerd via $2" ] || return 1
        [ "$(cat "$R/containers/$c/bin/ur-workertools")" = "bin ur-workertools via $2" ] || return 1
    done
    [ "$(cat "$R/containers/server/ur-server")" = "bin ur-server via $1" ]
}
t_stage() { (cd "$R" && sandbox_env bash scripts/build/stage.sh >/dev/null 2>&1) && staged_as zigbuild zigbuild; }
t_stage_native() { # UR_NATIVE_BUILD: ur-server still via zigbuild (image glibc), workers via a host build
    (cd "$R" && sandbox_env UR_NATIVE_BUILD=1 bash scripts/build/stage.sh >/dev/null 2>&1) || return 1
    staged_as zigbuild build || return 1
    grep -q '^cargo zigbuild --release --target [a-z0-9_]*-unknown-linux-gnu -p ur-server TAG' "$DSTATE/cargo-calls" \
        && grep -q '^cargo build --release -p workerd -p ur-workertools TAG' "$DSTATE/cargo-calls"
}
# A process holding a cargo build lock the way cargo does (flock on .cargo-lock).
hold_cargo_lock() {
    mkdir -p "$(dirname "$1")"; : > "$1"
    bash -c 'exec 8<"$1"; perl -MFcntl=:flock -e "open(my \$f, q(<&=), 8) or die; flock(\$f, LOCK_EX) or die"
             : > "$1.held"; while :; do sleep 0.2 8<&-; done' _ "$1" &
    BGPIDS+=("$!"); wait_file "$1.held"
}
housekeeping() { (cd "$R" && sandbox_env "$@" bash scripts/build/housekeeping.sh >/dev/null 2>"$S/hk.err"); }
t_housekeeping_sweeps_a_week() { housekeeping && grep -q "^cargo sweep --time 7 " "$DSTATE/cargo-calls"; }
t_housekeeping_skips_sweep_during_build() {
    local l
    for l in target/debug/.cargo-lock target/aarch64-unknown-linux-gnu/release/.cargo-lock; do
        : > "$DSTATE/cargo-calls"; rm -rf "$R/target"
        hold_cargo_lock "$R/$l" || return 1
        housekeeping || return 1
        grep -q "^cargo sweep" "$DSTATE/cargo-calls" && return 1
        grep -q "skipping cargo sweep" "$S/hk.err" || return 1
        kill_bg; BGPIDS=()
    done
    # idle lock files (no build running) do not block the sweep
    sleep 0.5; housekeeping && grep -q "^cargo sweep" "$DSTATE/cargo-calls"
}
t_housekeeping_warns_never_fails() { housekeeping DOCKER_PRUNE_RC=1 && grep -q "docker builder prune failed" "$S/hk.err"; }

# ---------------------------------------------------------------- pre-push --
t_hook_stale_cargo_lock() { # rejected before any `cargo make` step runs
    hook CARGO_LOCK_STALE=1 && return 1
    [ ! -s "$DSTATE/make-calls" ] && grep -q "Cargo.lock check" "$S/hook.out"
}
t_hook_ok()       { hook || return 1; [ -z "$(ls "$R/tmp" 2>/dev/null)" ] && grep -q "make ci" "$DSTATE/make-calls" \
                        && grep -q "make acceptance-ci" "$DSTATE/make-calls"; }
t_hook_ci_fail()  { hook MAKE_CI_RC=1 && return 1; ! grep -q "make acceptance-ci" "$DSTATE/make-calls" && [ -n "$(ls "$R/tmp")" ]; }
t_hook_mkdir_fail() { # trap is installed before mkdir: housekeeping runs, the push still fails
    echo x > "$R/tmp"; hook && return 1; grep -q "builder prune" "$DSTATE/calls"; }
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
SKIPS=()
COUNT=0
for t in $(declare -F | awk '{print $3}' | grep -E '^t_'); do
    echo "$t" | grep -Eq "$ONLY" || continue
    COUNT=$((COUNT + 1))
    reset_state || { echo "FAIL $t (setup)"; FAILS+=("$t"); continue; }
    # Each test runs in a subshell; it kills the background processes it started.
    start=$(now)
    ( trap kill_bg EXIT; "$t" ) >/dev/null 2>&1
    case $? in
        0) r="ok  " ;;
        77) r=skip; SKIPS+=("$t") ;;
        *) r=FAIL; FAILS+=("$t") ;;
    esac
    echo "$r $t ($(elapsed "$start")s)$([ "$r" = skip ] && echo ' (no collating locale)')"
    [ -n "${UR_CI_TEST_FAIL_FAST:-}" ] && [ ${#FAILS[@]} -gt 0 ] && break
done
echo "test-ci-scripts: $((COUNT - ${#FAILS[@]}))/$COUNT passed${SKIPS[*]:+ (skipped: ${SKIPS[*]})}"
[ ${#FAILS[@]} -eq 0 ]
