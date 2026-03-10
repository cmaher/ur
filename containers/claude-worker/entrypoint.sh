#!/bin/bash
set -e

# Ensure the Claude config directory exists.
# The credentials file is bind-mounted from the host at
# ~/.claude/.credentials.json — this just ensures the parent dir exists
# in case the mount creates it with wrong ownership.
mkdir -p ~/.claude
mkdir -p ~/.local/bin

# Start ur-workerd in background (creates command shims)
ur-workerd &

# Start a detached tmux session (no PTY required).
# The container stays alive via sleep; attach with `tmux attach -t agent`.
tmux -u new-session -d -s agent
exec sleep infinity
