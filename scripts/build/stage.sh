#!/usr/bin/env bash
set -euo pipefail

# shellcheck source-path=SCRIPTDIR
source "$(dirname "$0")/agent-images.sh"

# Build every Linux binary baked into the container images and stage each into
# its Docker build context:
#   ur-server               -> containers/server/
#   workerd, ur-workertools -> containers/<agent>/bin/
#
# Default: ONE cargo-zigbuild invocation (requires zig + cargo-zigbuild) for all
# three, targeting linux-gnu for the host arch so the binaries link against an
# old enough glibc for the debian bookworm images. One invocation matters:
# separate invocations resolve features independently, so shared dependencies
# (tokio, tonic, sqlx, ...) would be compiled once per invocation.
#
# UR_NATIVE_BUILD=1 (Linux hosts): ur-server is still built with zigbuild for
# the linux-gnu target -- a plain host build would link against the host's
# glibc, which can be newer than the image's -- and workerd + ur-workertools
# with a plain `cargo build` for the host target.

# Raise fd limit — the zig linker opens hundreds of .rlib files simultaneously
# and can exceed macOS's default soft limit (256).
ulimit -n 65536 2>/dev/null || true

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    x86_64)        TARGET="x86_64-unknown-linux-gnu" ;;
    *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

if [ "${UR_NATIVE_BUILD:-}" = "1" ]; then
    echo "Building ur-server for $TARGET and worker binaries natively"
    if command -v rustup >/dev/null 2>&1; then
        rustup target add "$TARGET" >/dev/null 2>&1 || true
    fi
    cargo zigbuild --release --target "$TARGET" -p ur-server
    cargo build --release -p workerd -p ur-workertools
    SERVER_OUT="target/$TARGET/release"
    WORKER_OUT=target/release
else
    echo "Cross-compiling image binaries for $TARGET"
    cargo zigbuild --release --target "$TARGET" -p ur-server -p workerd -p ur-workertools
    SERVER_OUT="target/$TARGET/release"
    WORKER_OUT="$SERVER_OUT"
fi

cp "$SERVER_OUT/ur-server" containers/server/ur-server
echo "Staged ur-server in containers/server/"

# Every agent's image build context needs its own copy of the worker binaries
# (each is an independent Docker build context).
for entry in "${AGENT_IMAGES[@]}"; do
    IFS=':' read -r dir _tag _agent _cachebust stage_binaries <<< "$entry"
    [ "$stage_binaries" = "true" ] || continue
    DEST="containers/$dir/bin"
    mkdir -p "$DEST"
    cp "$WORKER_OUT/ur-workertools" "$DEST/ur-workertools"
    cp "$WORKER_OUT/workerd" "$DEST/workerd"
    echo "Staged worker binaries in $DEST/"
done
