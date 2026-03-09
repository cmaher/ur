#!/bin/bash
exec /root/.local/bin/claude --dangerously-skip-permissions --disallowedTools "WebFetch WebSearch" "$@"
