#!/usr/bin/env bash
set -euo pipefail

# Rebuild the host CLI binaries (ur + builderd) without touching containers.
# Use this when you've only changed code in crates/ur/ or crates/builderd/.

echo "Building ur and builderd..."
cargo build -p ur -p builderd

echo "Host binaries rebuilt in target/debug/"
echo "  ur:       target/debug/ur"
echo "  builderd: target/debug/builderd"
