#!/usr/bin/env bash
set -euo pipefail

# Vendored wrapper that downloads and installs the native OpenAI Codex CLI
# release binaries. Cached in the build context (like vendor/claude/install.sh)
# so the Dockerfile doesn't reference a remote URL directly — only this
# script does, and it still hits the network at build time to fetch the
# current release.
#
# Codex ships musl-linked native binaries per architecture on GitHub
# Releases (https://github.com/openai/codex/releases) — no node runtime
# required, unlike the npm-distributed Claude CLI.
#
# Two binaries are installed, both resolved from the *same* release so they
# can never drift apart:
#
#   codex                 the CLI itself
#   codex-code-mode-host  companion process the CLI spawns for Code Mode
#
# The companion is not optional in practice: codex looks for it at a fixed
# path (/usr/local/bin/codex-code-mode-host) and, when it is missing, every
# session opens with "Code Mode is unavailable ... Code mode will fail
# closed", degrading tool use for the whole run.

REPO="openai/codex"
INSTALL_DIR="/usr/local/bin"

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
select_asset() {
    local jq_filter="$1"
    printf '%s' "$release_json" | jq -r --arg target "$target" --arg prefix "$2" "$jq_filter"
}

# Match the exact expected filename first: `<prefix>-<target>.tar.gz`. A
# looser predicate is not specific enough to be the primary rule — the
# release ships many prefix-sharing assets per target (`codex-app-server`,
# `codex-code-mode-host`, a bundled `bwrap` sandboxing helper, plus
# `.sigstore`/`.sha256`/`.zst` sidecars for every one of them), so a `[0]`
# pick out of a loose match set depends on GitHub's array order and could
# silently install the wrong binary.
#
# The fallback stays for an upstream extension change but is anchored on both
# ends: after stripping `<prefix>-`, the remainder must *start* with the
# target. Without that anchor, prefix "codex" would happily match
# `codex-code-mode-host-<target>.tar.gz` and install the companion as the CLI.
resolve_asset_url() {
    local prefix="$1"
    local url

    url="$(select_asset '[.assets[].browser_download_url
        | select((split("/") | last) as $name
            | ($name == $prefix + "-" + $target + ".tar.gz")
              or ($name == $prefix + "-" + $target + ".tgz"))
        ][0] // empty' "$prefix")"

    if [ -z "$url" ]; then
        url="$(select_asset '[.assets[].browser_download_url
            | select(
                (split("/") | last) as $name
                | ($name | startswith($prefix + "-"))
                and ($name | ltrimstr($prefix + "-") | startswith($target))
                and (($name | endswith(".tar.gz")) or ($name | endswith(".tgz")))
              )
            ][0] // empty' "$prefix")"
        if [ -n "$url" ]; then
            echo "WARNING: no asset named exactly $prefix-$target.tar.gz; falling back to the" >&2
            echo "         first $prefix-$target*.tar.gz asset: $url" >&2
            echo "         Check whether upstream renamed the release asset." >&2
        fi
    fi

    if [ -z "$url" ]; then
        echo "Could not find a '$prefix' release asset for target '$target' in the latest $REPO release" >&2
        echo "$release_json" >&2
        exit 1
    fi

    printf '%s' "$url"
}

# Verify a download against the release's own `<asset>.sha256` sidecar before
# installing it. These binaries run in a container with
# `sandbox_mode = "danger-full-access"`, and unlike vendor/claude/install.sh
# this script resolves and fetches its payload from the network at build
# time, so an unverified install is the one real supply-chain gap in this
# image. A missing/unparseable sidecar warns rather than failing the build
# (upstream may change the sidecar layout), but a *mismatch* is fatal.
verify_checksum() {
    local asset_url="$1" archive="$2" sidecar="$3"
    local sidecar_url="$asset_url.sha256"
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

# Download, verify, extract and install one release binary named `<prefix>`
# to $INSTALL_DIR/<prefix>.
install_release_binary() {
    local prefix="$1"
    local asset_url tmp_dir dl_dir extract_dir archive matches binary_path

    asset_url="$(resolve_asset_url "$prefix")"

    tmp_dir="$(mktemp -d)"
    # shellcheck disable=SC2064  # expand tmp_dir now, not at trap time
    trap "rm -rf '$tmp_dir'" RETURN

    # Download and extract into separate subdirectories: the extracted tree is
    # then the only thing the binary search below walks, so neither the archive
    # nor its checksum sidecar can be mistaken for the binary.
    dl_dir="$tmp_dir/download"
    extract_dir="$tmp_dir/extract"
    mkdir -p "$dl_dir" "$extract_dir"

    archive="$dl_dir/archive"
    echo "Downloading $asset_url"
    curl -fsSL -o "$archive" "$asset_url"
    verify_checksum "$asset_url" "$archive" "$dl_dir/archive.sha256"

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
    # directory; search rather than assume a fixed layout. Prefer the exact
    # names the archive is expected to use, so a companion binary shipped
    # alongside it can never win on `find` order; fall back to a sorted
    # (deterministic) pick and say so.
    #
    # Each search collects the whole sorted list and then takes its first line,
    # rather than piping into `head -n1`: `head` closing the pipe early kills
    # `sort` with SIGPIPE, which `set -o pipefail` turns into a bare exit 141
    # that aborts the build with no message once the list outgrows the 64KB
    # pipe buffer.
    matches="$(find "$extract_dir" -type f \( -name "$prefix" -o -name "$prefix-$target" \) | sort)"
    binary_path="${matches%%$'\n'*}"
    if [ -z "$binary_path" ]; then
        matches="$(find "$extract_dir" -type f -name "$prefix*" | sort)"
        binary_path="${matches%%$'\n'*}"
        if [ -n "$binary_path" ]; then
            echo "WARNING: no file named '$prefix' or '$prefix-$target' in the archive; falling back" >&2
            echo "         to $binary_path" >&2
        fi
    fi
    if [ -z "$binary_path" ]; then
        echo "Could not locate a '$prefix' binary after extracting $asset_url" >&2
        exit 1
    fi

    # Install as root — `install`'s destination-replace step unlinks the
    # existing file first, which needs write access to the (root-owned)
    # directory itself, not just the file.
    install -m 0755 "$binary_path" "$INSTALL_DIR/$prefix"
    echo "Installed $prefix to $INSTALL_DIR/$prefix"
}

install_release_binary codex
install_release_binary codex-code-mode-host

"$INSTALL_DIR/codex" --version

# The companion has no stable `--version` contract, so assert what codex
# itself needs — that the path it spawns exists and is executable — rather
# than running it.
if [ ! -x "$INSTALL_DIR/codex-code-mode-host" ]; then
    echo "codex-code-mode-host is not executable at $INSTALL_DIR/codex-code-mode-host" >&2
    exit 1
fi
echo "codex-code-mode-host installed at $INSTALL_DIR/codex-code-mode-host"
