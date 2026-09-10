#!/bin/bash
set -e

mkdir -p ~/.gemini/antigravity-cli/cache ~/.gemini/config ~/.local/bin

workerd init
(cd /workspace && [ -f Cargo.toml ] && cargo sweep --time 1 &) 2>/dev/null
(cd /workspace && bacon --headless ai &)
exec workerd daemon
