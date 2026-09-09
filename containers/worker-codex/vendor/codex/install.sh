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
# A plain `contains($target)` match is not enough: the release also ships a
# bundled `bwrap-<target>...` sandboxing helper plus `.sigstore`/`.sha256`
# sidecar files for every asset, all of which contain the target string too.
# Scope to the codex CLI's own archive by requiring the asset's filename to
# start with "codex-" and end in a supported archive extension.
asset_url="$(printf '%s' "$release_json" \
    | jq -r --arg target "$target" \
        '[.assets[].browser_download_url
            | select(
                (split("/") | last) as $name
                | ($name | startswith("codex-"))
                and ($name | contains($target))
                and (($name | endswith(".tar.gz")) or ($name | endswith(".tgz")))
              )
         ][0] // empty')"

if [ -z "$asset_url" ]; then
    echo "Could not find a release asset for target '$target' in the latest $REPO release" >&2
    echo "$release_json" >&2
    exit 1
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

archive="$tmp_dir/codex-archive"
echo "Downloading $asset_url"
curl -fsSL -o "$archive" "$asset_url"

case "$asset_url" in
    *.tar.gz|*.tgz)
        tar -xzf "$archive" -C "$tmp_dir"
        ;;
    *)
        echo "Unrecognized archive format for $asset_url (expected .tar.gz/.tgz)" >&2
        exit 1
        ;;
esac

# The binary may be at the archive root or nested under a version-named
# directory; search rather than assume a fixed layout. Exclude the archive
# itself in case its own name also matches "codex*".
binary_path="$(find "$tmp_dir" -type f -name 'codex*' ! -path "$archive" | head -n1)"
if [ -z "$binary_path" ]; then
    echo "Could not locate a codex binary after extracting $asset_url" >&2
    exit 1
fi

install -m 0755 "$binary_path" "$INSTALL_PATH"
echo "Installed codex to $INSTALL_PATH"
"$INSTALL_PATH" --version
