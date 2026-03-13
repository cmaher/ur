#!/bin/bash
set -e

# Activate mise so Rust toolchain and bacon are on PATH
eval "$(/home/worker/.local/bin/mise activate bash)"

# Ensure the Claude config directory exists.
# The credentials file is bind-mounted from the host at
# ~/.claude/.credentials.json — this just ensures the parent dir exists
# in case the mount creates it with wrong ownership.
mkdir -p ~/.claude
mkdir -p ~/.local/bin

# Run all container initialization (skills, git hooks)
ur-tools init

# Start ur-workerd in background (creates command shims)
ur-workerd &

# Start bacon in headless mode for background compilation checking.
# Uses the 'ai' job (cargo check --message-format short) and exports
# diagnostics to .bacon-locations for agents to read.
# Runs from /workspace where the project and bacon.toml live.
(cd /workspace && bacon --headless ai &)

# Start a detached tmux session (no PTY required).
# The container stays alive via sleep; attach with `tmux attach -t agent`.
# Use -x/-y to set a large default size since there's no client at creation time.
# When a real client attaches, tmux resizes to the client's terminal dimensions.
tmux -u new-session -d -s agent -x 220 -y 55
exec sleep infinity
