#!/usr/bin/env bash
set -euo pipefail

# Print a short content hash of everything that goes into the container images:
# the Rust crates the image binaries depend on, proto/ (compiled by ur_rpc's
# build.rs), Cargo.toml/Cargo.lock, every container build context (including
# submodule working trees), the build scripts, and the toolchain, build
# settings and arch.
#
# Two checkouts with the same inputs produce the same hash, so acceptance-ci can
# tag images `ci-<hash>` and skip both the release build and `docker build` when
# those images already exist. Uses working-tree contents (what the build would
# actually see), not the index. The staged build outputs (containers/server/
# ur-server, containers/*/bin/) are excluded: they are derived from the hashed
# sources.

# Byte-order sorting: the hash must not depend on the caller's locale.
export LC_ALL=C

cd "$(dirname "$0")/../.."

if command -v sha256sum >/dev/null 2>&1; then
    SHA=(sha256sum)
else
    SHA=(shasum -a 256)
fi

# cargo's view of the workspace root, which is what `cargo tree` prints paths
# relative to (it can differ from $PWD, e.g. when cargo runs via a host shim).
cargo_root=$(cargo metadata --locked --no-deps --format-version 1 \
    | sed -n 's#.*"workspace_root":"\([^"]*\)".*#\1#p')

# Lists are collected with plain command substitution (not `< <(...)`), so a
# failing cargo/git under pipefail fails the script instead of silently
# producing a partial list -- and therefore a wrong hash.

# Workspace crates in the normal/build dependency closure of the image binaries.
# Path deps print as `name vX (/abs/path)`; registry and git deps never match.
crate_list=$(
    cargo tree --quiet --locked -e normal,build --prefix none --format '{p}' --target all \
        -p ur-server -p workerd -p ur-workertools \
        | sed -n "s#.*(${cargo_root}/\([^)]*\)).*#\1#p" \
        | sort -u
)
crate_dirs=()
while IFS= read -r dir; do
    [ -n "$dir" ] && crate_dirs+=("$dir")
done <<< "$crate_list"
if [ "${#crate_dirs[@]}" -eq 0 ]; then
    echo "image-hash: cargo tree found no workspace crates" >&2
    exit 1
fi

# Hash every regular file named on stdin (NUL-separated), relative to $PWD.
# Tracked-but-deleted paths are skipped; their absence still changes the hash.
hash_files() {
    local path list=()
    while IFS= read -r -d '' path; do
        [ -f "$path" ] && list+=("$path")
    done
    if [ "${#list[@]}" -gt 0 ]; then
        "${SHA[@]}" "${list[@]}"
    fi
}

# Docker build contexts have no .dockerignore, so `docker build` sends files that
# git ignores (repo or global gitignore). List containers/ WITHOUT
# --exclude-standard, minus the staged build outputs. Crates, proto and scripts
# keep --exclude-standard: cargo does not read ignored files there.
STAGED_OUTPUTS='^containers/server/ur-server$|^containers/server/builderd$|^containers/[^/]+/bin/'

# `&&` between the two listings: errexit is off inside command substitution
# (bash < 4.4 has no inherit_errexit), so a failure of the first would
# otherwise be masked by the second's status.
path_list=$(
    {
        git ls-files -co --exclude-standard -- \
            Cargo.toml Cargo.lock rust-toolchain rust-toolchain.toml \
            proto scripts/build "${crate_dirs[@]}" \
            && git ls-files -co -- containers | { grep -Ev "$STAGED_OUTPUTS" || true; }
    } | sort -u
)
files=()
submodules=()
while IFS= read -r path; do
    if [ -d "$path" ]; then
        submodules+=("$path")
    elif [ -f "$path" ]; then
        files+=("$path")
    fi
done <<< "$path_list"
if [ "${#files[@]}" -eq 0 ]; then
    echo "image-hash: git ls-files found no input files" >&2
    exit 1
fi

# Submodule working-tree contents: names and bytes of every file, tracked or
# not, so two different uncommitted edits never collide. Inside containers/ the
# whole tree is listed (it is a Docker build context); elsewhere ignored files
# are skipped. An uninitialized submodule has no repo of its own (its
# show-prefix is non-empty) and contributes only its recorded commit.
hash_submodule() {
    local sub="$1" ls_args=(-co -z)
    case "$sub" in
        containers/*) ;;
        *) ls_args+=(--exclude-standard) ;;
    esac
    # Status flag + recorded/checked-out commit + path. The trailing
    # `(git describe)` is dropped: it changes when tags are fetched.
    git submodule status -- "$sub" 2>/dev/null | sed 's/ (.*)$//' \
        || echo "nested-repo $sub $(git -C "$sub" rev-parse HEAD 2>/dev/null || echo none)"
    if [ -z "$(git -C "$sub" rev-parse --show-prefix 2>/dev/null || echo uninit)" ]; then
        echo "submodule-files $sub"
        (cd "$sub" && git ls-files "${ls_args[@]}" | hash_files)
    fi
}

# Build settings that change the binaries without changing any source file:
# rustc flags (RUSTFLAGS, CARGO_ENCODED_RUSTFLAGS), profile/build/target
# settings and the docker platform. Scoped to variables that change the OUTPUT,
# so the hash is the same from a plain shell and from inside `cargo make`
# (which sets CARGO_MAKE_*, CARGO, ...). Where the output goes
# (CARGO_TARGET_DIR, CARGO_BUILD_TARGET_DIR) and how many jobs build it
# (CARGO_BUILD_JOBS) do not count.
build_settings() {
    echo "arch $(uname -m)"
    echo "native ${UR_NATIVE_BUILD:-}"
    cargo -V
    rustc -vV
    if command -v protoc >/dev/null 2>&1; then
        protoc --version
    fi
    # zigbuild builds ur-server in both modes (stage.sh), and everything by default.
    if command -v zig >/dev/null 2>&1; then
        echo "zig $(zig version)"
    fi
    # (`cargo zigbuild` has no --version; the binary cargo dispatches to does.)
    if command -v cargo-zigbuild >/dev/null 2>&1; then
        cargo-zigbuild --version
    fi
    # Cargo config: the repo's, plus the user-level one cargo also reads.
    local cfg cargo_home="${CARGO_HOME:-$HOME/.cargo}"
    for cfg in mise.toml .cargo/config.toml "$cargo_home/config.toml" "$cargo_home/config"; do
        if [ -f "$cfg" ]; then
            echo "config ${cfg#"$cargo_home"/}"
            "${SHA[@]}" < "$cfg"
        fi
    done
    env \
        | { grep -E '^(RUSTFLAGS=|CARGO_ENCODED_RUSTFLAGS=|CARGO_PROFILE_|CARGO_BUILD_|CARGO_TARGET_|DOCKER_DEFAULT_PLATFORM=)' || true; } \
        | { grep -Ev '^(CARGO_TARGET_DIR|CARGO_BUILD_TARGET_DIR|CARGO_BUILD_JOBS)=' || true; } \
        | sort
}

{
    build_settings
    printf '%s\0' "${files[@]}" | hash_files
    if [ "${#submodules[@]}" -gt 0 ]; then
        for sub in "${submodules[@]}"; do
            hash_submodule "$sub"
        done
    fi
} | "${SHA[@]}" | cut -c1-12
