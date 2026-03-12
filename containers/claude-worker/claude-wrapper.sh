#!/bin/bash
exec /home/worker/.local/bin/claude --dangerously-skip-permissions "$@"
