#!/usr/bin/env bash
set -euo pipefail

# Print a short content hash of everything that goes into the container images:
# the Rust crates the image binaries depend on, Cargo.toml/Cargo.lock, every
# container build context, the build scripts, and the toolchain/arch.
#
# Two checkouts with the same inputs produce the same hash, so acceptance-ci can
# tag images `ci-<hash>` and skip both the release build and `docker build` when
# those images already exist. Uses working-tree contents (what the build would
# actually see), not the index. Staged binaries are gitignored and excluded.

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
        -p ur-server -p workerd -p workertools 2>/dev/null \
        | sed -n "s#.*(${cargo_root}/\([^)]*\)).*#\1#p" \
        | sort -u
)
if [ "${#crate_dirs[@]}" -eq 0 ]; then
    echo "image-hash: cargo tree found no workspace crates" >&2
    exit 1
fi

files=()
submodules=()
while IFS= read -r path; do
    if [ -d "$path" ]; then
        submodules+=("$path")
    elif [ -f "$path" ]; then
        files+=("$path")
    fi
done < <(
    git ls-files -co --exclude-standard -- \
        Cargo.toml Cargo.lock rust-toolchain rust-toolchain.toml \
        scripts/build containers "${crate_dirs[@]}" \
        | sort -u
)

{
    echo "arch $(uname -m)"
    echo "native ${UR_NATIVE_BUILD:-}"
    cargo -V
    printf '%s\0' "${files[@]}" | xargs -0 "${SHA[@]}"
    # Submodules: checked-out commit plus whether their contents are dirty.
    if [ "${#submodules[@]}" -gt 0 ]; then
        git submodule status -- "${submodules[@]}"
        git status --porcelain --ignore-submodules=none -- "${submodules[@]}"
    fi
} | "${SHA[@]}" | cut -c1-12
