#!/usr/bin/env bash
set -euo pipefail

# Build the ur CLI and builderd binaries and install them.
# ur-server is started via `ur start`.
# Set UR_BUILD_PROFILE=debug for debug builds (default: release).
# Set UR_INSTALL_DIR to override the install directory (default: ~/.local/bin).

PROFILE="${UR_BUILD_PROFILE:-release}"
INSTALL_DIR="${UR_INSTALL_DIR:-$HOME/.local/bin}"

if [ "$PROFILE" = "debug" ]; then
    cargo build -p ur -p builderd
    TARGET_DIR="target/debug"
else
    cargo build --release -p ur -p builderd
    TARGET_DIR="target/release"
fi

# Kill existing daemon before replacing binary
if [ -x "$INSTALL_DIR/ur" ]; then
    "$INSTALL_DIR/ur" kill server 2>/dev/null || true
fi

mkdir -p "$INSTALL_DIR" "$HOME/.ur/logs"

# Remove before copying: macOS caches code signature page hashes for running
# binaries. Overwriting in-place (cp) invalidates the cache, causing the kernel
# to SIGKILL the new binary on exec. Removing first creates a new inode.
rm -f "$INSTALL_DIR/ur" "$INSTALL_DIR/builderd"
cp "$TARGET_DIR/ur" "$INSTALL_DIR/ur"
cp "$TARGET_DIR/builderd" "$INSTALL_DIR/builderd"
echo "Installed ur and builderd to $INSTALL_DIR/"
echo "Run 'ur start' to launch the server"

# Download the default embedding model for RAG if not already cached.
# This matches the hf_hub cache layout fastembed expects.
FASTEMBED_DIR="${UR_CONFIG:-$HOME/.ur}/fastembed"
MODEL_DIR="$FASTEMBED_DIR/models--Qdrant--all-MiniLM-L6-v2-onnx"
COMMIT="5f1b8cd78bc4fb444dd171e59b18f3a3af89a079"
SNAPSHOT_DIR="$MODEL_DIR/snapshots/$COMMIT"

if [ -d "$SNAPSHOT_DIR" ] && [ -f "$SNAPSHOT_DIR/model.onnx" ]; then
    echo "Embedding model already cached at $FASTEMBED_DIR"
else
    echo "Downloading embedding model (all-MiniLM-L6-v2)..."
    mkdir -p "$MODEL_DIR/refs" "$MODEL_DIR/blobs" "$SNAPSHOT_DIR"
    echo -n "$COMMIT" > "$MODEL_DIR/refs/main"
    HF_BASE="https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main"
    for f in model.onnx tokenizer.json config.json special_tokens_map.json tokenizer_config.json; do
        curl -fSL -o "$SNAPSHOT_DIR/$f" "$HF_BASE/$f"
    done
    echo "Embedding model cached at $FASTEMBED_DIR"
fi
