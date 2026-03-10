#!/usr/bin/env bash
set -euo pipefail

# Rebuild the host CLI binaries (ur + ur-hostd) without touching containers.
# Use this when you've only changed code in crates/ur/ or crates/hostd/.

echo "Building ur and ur-hostd..."
cargo build -p ur -p ur-hostd

echo "Host binaries rebuilt in target/debug/"
echo "  ur:       target/debug/ur"
echo "  ur-hostd: target/debug/ur-hostd"
