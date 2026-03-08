#!/bin/bash
set -e

# Write Claude credentials to the expected config path if provided.
if [ -n "$CLAUDE_CREDENTIALS" ]; then
    mkdir -p ~/.claude
    printf '%s' "$CLAUDE_CREDENTIALS" > ~/.claude/.credentials.json
    unset CLAUDE_CREDENTIALS
fi

# Start a detached tmux session (no PTY required).
# The container stays alive via sleep; attach with `tmux attach -t agent`.
tmux new-session -d -s agent
exec sleep infinity
