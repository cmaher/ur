#!/usr/bin/env bash
set -euo pipefail

# Build urd binary natively and stage for Dockerfile (for Linux CI).

cargo build --release -p urd

DEST=containers/urd
cp target/release/urd "$DEST/urd"

echo "Staged urd binary in $DEST/"
