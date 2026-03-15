#!/bin/bash
set -e

# Ensure the Claude config directory exists.
# The credentials file is bind-mounted from the host at
# ~/.claude/.credentials.json — this just ensures the parent dir exists
# in case the mount creates it with wrong ownership.
mkdir -p ~/.claude
mkdir -p ~/.local/bin

# Initialize: skills, git hooks, hostexec shims (synchronous)
workerd init

# Start daemon in background
workerd &

# Sweep stale cargo build artifacts in the background (best-effort).
# target/ persists across workers in pool slots, so old artifacts accumulate.
(cd /workspace && [ -f Cargo.toml ] && cargo sweep --time 1 &) 2>/dev/null

# Start bacon on the host via hostexec shim.
# Uses the 'ai' job (cargo check --message-format short) and exports
# diagnostics to .bacon-locations for agents to read.
(cd /workspace && bacon --headless ai &)

# Start a detached tmux session (no PTY required).
# The container stays alive via sleep; attach with `tmux attach -t agent`.
# Use -x/-y to set a large default size since there's no client at creation time.
# When a real client attaches, tmux resizes to the client's terminal dimensions.
tmux -u new-session -d -s agent -x 220 -y 55
exec sleep infinity
