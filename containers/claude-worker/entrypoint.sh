#!/bin/sh
set -e

# Start tmux in foreground (keeps container alive)
tmux new-session -s agent
