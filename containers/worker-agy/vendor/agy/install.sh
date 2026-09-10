#!/bin/sh
set -eu
set -o pipefail

MANIFEST_BASE="https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests"
INSTALL_DIR="$HOME/.local/bin"

while [ "$#" -gt 0 ]; do
    case "$1" in
        --dir)
            [ "$#" -ge 2 ] || { echo "--dir requires a path" >&2; exit 1; }
            INSTALL_DIR="$2"
            shift 2
            ;;
        *)
            echo "unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

detect_platform() {
    local arch platform
    case "$(uname -m)" in
        x86_64|amd64) arch="amd64" ;;
        aarch64|arm64) arch="arm64" ;;
        *) echo "unsupported architecture: $(uname -m)" >&2; exit 1 ;;
    esac

    platform="linux_${arch}"
    if ldd /bin/sh 2>&1 | grep -q musl; then
        platform="${platform}_musl"
    fi
    printf '%s\n' "$platform"
}

json_string() {
    local key="$1" payload="$2"
    printf '%s' "$payload" |
        sed -n 's/.*"'"$key"'"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p'
}

platform="$(detect_platform)"
manifest_url="${MANIFEST_BASE}/${platform}.json"
manifest="$(curl -fsSL "$manifest_url")" || {
    echo "failed to fetch AGY manifest for ${platform}" >&2
    exit 1
}

version="$(json_string version "$manifest")"
url="$(json_string url "$manifest")"
sha512="$(json_string sha512 "$manifest")"
[ -n "$version" ] && [ -n "$url" ] && [ -n "$sha512" ] || {
    echo "invalid AGY manifest for ${platform}" >&2
    exit 1
}
case "$url" in
    https://storage.googleapis.com/antigravity-public/*) ;;
    *) echo "refusing untrusted AGY download URL" >&2; exit 1 ;;
esac

staging_dir="$(mktemp -d)"
trap 'rm -rf "$staging_dir"' EXIT INT TERM
archive="$staging_dir/agy.tar.gz"
curl -fsSL "$url" -o "$archive"
printf '%s  %s\n' "$sha512" "$archive" | sha512sum -c -
tar -xzf "$archive" -C "$staging_dir" antigravity

mkdir -p "$INSTALL_DIR"
install -m 0755 "$staging_dir/antigravity" "$INSTALL_DIR/agy"
"$INSTALL_DIR/agy" install || true
printf 'Installed AGY %s to %s/agy\n' "$version" "$INSTALL_DIR"
