#!/bin/bash
set -e

mkdir -p ~/.codex
mkdir -p ~/.local/bin

# workerd handles init (skills, git hooks, shims), tmux, codex, and gRPC.
# exec makes workerd PID 1 — container lifecycle is tied to it.
exec workerd
