#!/bin/bash
set -e

mkdir -p ~/.claude
mkdir -p ~/.local/bin

# workerd handles init (skills, git hooks, shims), tmux, claude, and gRPC.
# exec makes workerd PID 1 — container lifecycle is tied to it.
exec workerd
