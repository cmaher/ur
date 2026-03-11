#!/bin/bash
exec /home/worker/.local/bin/claude-real --dangerously-skip-permissions "$@"
