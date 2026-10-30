#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/agent-images.sh"

# Build every Linux binary baked into the container images and stage each into
# its Docker build context:
#   ur-server              -> containers/server/
#   workerd, workertools   -> containers/<agent>/bin/
#
# All packages build in ONE cargo invocation. Separate invocations resolve
# features independently, so shared dependencies (tokio, tonic, sqlx, ...) would
# be compiled once per invocation.
#
# Default: cross-compile with cargo-zigbuild (requires zig + cargo-zigbuild),
# targeting linux-gnu for the host arch to match the debian bookworm images.
# UR_NATIVE_BUILD=1: plain `cargo build` for the host target (Linux hosts only).

# Raise fd limit — the zig linker opens hundreds of .rlib files simultaneously
# and can exceed macOS's default soft limit (256).
ulimit -n 65536 2>/dev/null || true

PACKAGES=(-p ur-server -p workerd -p workertools)

if [ "${UR_NATIVE_BUILD:-}" = "1" ]; then
    echo "Building image binaries natively"
    cargo build --release "${PACKAGES[@]}"
    OUT=target/release
else
    ARCH=$(uname -m)
    case "$ARCH" in
        arm64|aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
        x86_64)        TARGET="x86_64-unknown-linux-gnu" ;;
        *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
    esac
    echo "Cross-compiling image binaries for $TARGET"
    cargo zigbuild --release --target "$TARGET" "${PACKAGES[@]}"
    OUT="target/$TARGET/release"
fi

cp "$OUT/ur-server" containers/server/ur-server
echo "Staged ur-server in containers/server/"

# Every agent's image build context needs its own copy of the worker binaries
# (each is an independent Docker build context).
for entry in "${AGENT_IMAGES[@]}"; do
    IFS=':' read -r dir _tag _agent _cachebust stage_binaries <<< "$entry"
    [ "$stage_binaries" = "true" ] || continue
    DEST="containers/$dir/bin"
    mkdir -p "$DEST"
    cp "$OUT/workertools" "$DEST/workertools"
    cp "$OUT/workerd" "$DEST/workerd"
    echo "Staged worker binaries in $DEST/"
done
