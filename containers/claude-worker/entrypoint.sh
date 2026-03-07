#!/bin/bash
set -e

# Start a detached tmux session (no PTY required).
# The container stays alive via sleep; attach with `tmux attach -t agent`.
tmux new-session -d -s agent
exec sleep infinity
