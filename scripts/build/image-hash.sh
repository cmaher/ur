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

# Workspace crates in the normal/build dependency closure of the image binaries.
# Path deps print as `name vX (/abs/path)`; registry and git deps never match.
crate_dirs=()
while IFS= read -r dir; do
    crate_dirs+=("$dir")
done < <(
    cargo tree --locked -e normal,build --prefix none --format '{p}' --target all \
        -p ur-server -p workerd -p ur-workertools 2>/dev/null \
        | sed -n "s#.*(${cargo_root}/\([^)]*\)).*#\1#p" \
        | sort -u
)
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

files=()
submodules=()
while IFS= read -r path; do
    if [ -d "$path" ]; then
        submodules+=("$path")
    elif [ -f "$path" ]; then
        files+=("$path")
    fi
done < <(
    {
        git ls-files -co --exclude-standard -- \
            Cargo.toml Cargo.lock rust-toolchain rust-toolchain.toml \
            proto scripts/build "${crate_dirs[@]}"
        git ls-files -co -- containers | { grep -Ev "$STAGED_OUTPUTS" || true; }
    } | sort -u
)

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
    git submodule status -- "$sub" 2>/dev/null || echo "nested-repo $sub"
    if [ -z "$(git -C "$sub" rev-parse --show-prefix 2>/dev/null || echo uninit)" ]; then
        echo "submodule-files $sub"
        (cd "$sub" && git ls-files "${ls_args[@]}" | hash_files)
    fi
}

# Build settings that change the binaries without changing any source file.
# Scoped to variables cargo/docker actually read so the hash is the same from a
# plain shell and from inside `cargo make` (which sets CARGO_MAKE_*, CARGO, ...).
build_settings() {
    echo "arch $(uname -m)"
    echo "native ${UR_NATIVE_BUILD:-}"
    cargo -V
    rustc -vV
    if command -v protoc >/dev/null 2>&1; then
        protoc --version
    fi
    if [ "${UR_NATIVE_BUILD:-}" != "1" ] && command -v zig >/dev/null 2>&1; then
        echo "zig $(zig version)"
    fi
    local cfg
    for cfg in mise.toml .cargo/config.toml; do
        if [ -f "$cfg" ]; then
            "${SHA[@]}" "$cfg"
        fi
    done
    env | { grep -E '^(RUSTFLAGS=|CARGO_PROFILE_|CARGO_BUILD_|CARGO_TARGET_|DOCKER_DEFAULT_PLATFORM=)' || true; } | sort
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
