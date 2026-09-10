#!/usr/bin/env bash
set -euo pipefail

source "$(dirname "$0")/agent-images.sh"

# Cross-compile worker binaries for linux-gnu and stage for Dockerfile
# Requires: zig + cargo-zigbuild

ARCH=$(uname -m)
case "$ARCH" in
    arm64|aarch64) TARGET="aarch64-unknown-linux-gnu" ;;
    x86_64)        TARGET="x86_64-unknown-linux-gnu" ;;
    *)             echo "Unsupported architecture: $ARCH" >&2; exit 1 ;;
esac

echo "Cross-compiling worker binaries for $TARGET"
cargo zigbuild --release --target "$TARGET" -p ur-ping -p workertools -p workerd

# Every agent's base image build context needs its own copy of these
# binaries (each is an independent Docker build context).
for entry in "${AGENT_IMAGES[@]}"; do
    IFS=':' read -r dir _tag _agent _cachebust stage_binaries <<< "$entry"
    [ "$stage_binaries" = "true" ] || continue
    DEST="containers/$dir/bin"
    mkdir -p "$DEST"
    cp "target/$TARGET/release/ur-ping" "$DEST/ur-ping"
    cp "target/$TARGET/release/workertools" "$DEST/workertools"
    cp "target/$TARGET/release/workerd" "$DEST/workerd"
    echo "Staged worker binaries in $DEST/"
done
