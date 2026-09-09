#!/usr/bin/env bash
set -euo pipefail

# Vendored wrapper that downloads and installs the native OpenAI Codex CLI
# release binary. Cached in the build context (like vendor/claude/install.sh)
# so the Dockerfile doesn't reference a remote URL directly — only this
# script does, and it still hits the network at build time to fetch the
# current release.
#
# Codex ships a musl-linked native binary per architecture on GitHub
# Releases (https://github.com/openai/codex/releases) — no node runtime
# required, unlike the npm-distributed Claude CLI.

REPO="openai/codex"
INSTALL_PATH="/usr/local/bin/codex"

arch="$(uname -m)"
case "$arch" in
    x86_64)
        target="x86_64-unknown-linux-musl"
        ;;
    aarch64|arm64)
        target="aarch64-unknown-linux-musl"
        ;;
    *)
        echo "Unsupported architecture: $arch" >&2
        exit 1
        ;;
esac

api_url="https://api.github.com/repos/$REPO/releases/latest"
echo "Resolving latest codex release from $api_url"
release_json="$(curl -fsSL -H "Accept: application/vnd.github+json" "$api_url")"

# jq is already part of the base image (worker-base/Dockerfile), so parse
# the release JSON properly rather than scraping it with grep/sed.
#
# Match the exact expected filename first: `codex-<target>.tar.gz`. A looser
# `startswith("codex-") and contains($target)` predicate is not specific
# enough to be the primary rule — the release ships several codex-prefixed
# assets per target (a bundled `bwrap-<target>` sandboxing helper, companion
# binaries, plus `.sigstore`/`.sha256` sidecars for every asset), so a
# `[0]` pick out of a loose match set depends on GitHub's array order and
# could silently install the wrong binary. The loose predicate stays as a
# fallback for an upstream naming change, but says so loudly.
select_asset() {
    local jq_filter="$1"
    printf '%s' "$release_json" | jq -r --arg target "$target" "$jq_filter"
}

asset_url="$(select_asset '[.assets[].browser_download_url
    | select((split("/") | last) as $name
        | ($name == "codex-" + $target + ".tar.gz")
          or ($name == "codex-" + $target + ".tgz"))
    ][0] // empty')"

if [ -z "$asset_url" ]; then
    asset_url="$(select_asset '[.assets[].browser_download_url
        | select(
            (split("/") | last) as $name
            | ($name | startswith("codex-"))
            and ($name | contains($target))
            and (($name | endswith(".tar.gz")) or ($name | endswith(".tgz")))
          )
        ][0] // empty')"
    if [ -n "$asset_url" ]; then
        echo "WARNING: no asset named exactly codex-$target.tar.gz; falling back to the" >&2
        echo "         first codex-*$target*.tar.gz asset: $asset_url" >&2
        echo "         Check whether upstream renamed the release asset." >&2
    fi
fi

if [ -z "$asset_url" ]; then
    echo "Could not find a release asset for target '$target' in the latest $REPO release" >&2
    echo "$release_json" >&2
    exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

# Download and extract into separate subdirectories: the extracted tree is
# then the only thing the binary search below walks, so neither the archive
# nor its checksum sidecar can be mistaken for the binary.
dl_dir="$tmp_dir/download"
extract_dir="$tmp_dir/extract"
mkdir -p "$dl_dir" "$extract_dir"

archive="$dl_dir/codex-archive"
echo "Downloading $asset_url"
curl -fsSL -o "$archive" "$asset_url"

# Verify the download against the release's own `<asset>.sha256` sidecar
# before installing it. This binary runs in a container with
# `sandbox_mode = "danger-full-access"`, and unlike vendor/claude/install.sh
# this script resolves and fetches its payload from the network at build
# time, so an unverified install is the one real supply-chain gap in this
# image. A missing/unparseable sidecar warns rather than failing the build
# (upstream may change the sidecar layout), but a *mismatch* is fatal.
verify_checksum() {
    local sidecar_url="$asset_url.sha256"
    local sidecar="$dl_dir/codex-archive.sha256"
    if ! curl -fsSL -o "$sidecar" "$sidecar_url" 2>/dev/null; then
        echo "WARNING: no checksum sidecar at $sidecar_url — installing an unverified binary" >&2
        return 0
    fi
    local expected actual
    expected="$(tr -s '[:space:]' '\n' < "$sidecar" | grep -Eox '[0-9a-f]{64}' | head -n1 || true)"
    if [ -z "$expected" ]; then
        echo "WARNING: $sidecar_url holds no sha256 digest — installing an unverified binary" >&2
        return 0
    fi
    actual="$(sha256sum "$archive" | cut -d' ' -f1)"
    if [ "$expected" != "$actual" ]; then
        echo "Checksum mismatch for $asset_url" >&2
        echo "  expected: $expected" >&2
        echo "  actual:   $actual" >&2
        exit 1
    fi
    echo "Checksum verified: $actual"
}
verify_checksum

case "$asset_url" in
    *.tar.gz|*.tgz)
        tar -xzf "$archive" -C "$extract_dir"
        ;;
    *)
        echo "Unrecognized archive format for $asset_url (expected .tar.gz/.tgz)" >&2
        exit 1
        ;;
esac

# The binary may be at the archive root or nested under a version-named
# directory; search rather than assume a fixed layout. Prefer the exact names
# the archive is expected to use, so a companion binary shipped alongside it
# can never win on `find` order; fall back to a sorted (deterministic) pick
# and say so.
binary_path="$(find "$extract_dir" -type f \( -name codex -o -name "codex-$target" \) \
    | sort | head -n1)"
if [ -z "$binary_path" ]; then
    binary_path="$(find "$extract_dir" -type f -name 'codex*' | sort | head -n1)"
    if [ -n "$binary_path" ]; then
        echo "WARNING: no file named 'codex' or 'codex-$target' in the archive; falling back" >&2
        echo "         to $binary_path" >&2
    fi
fi
if [ -z "$binary_path" ]; then
    echo "Could not locate a codex binary after extracting $asset_url" >&2
    exit 1
fi

install -m 0755 "$binary_path" "$INSTALL_PATH"
echo "Installed codex to $INSTALL_PATH"
"$INSTALL_PATH" --version
