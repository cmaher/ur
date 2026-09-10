#!/bin/bash
set -e

mkdir -p ~/.gemini/antigravity-cli/cache ~/.gemini/config ~/.local/bin

# workerd handles init, tmux, AGY, and gRPC. exec makes it PID 1.
exec workerd
