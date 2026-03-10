#!/usr/bin/env bash
set -euo pipefail

# Rebuild and restart ur-hostd on the host.
# ur-hostd runs natively (not in a container), so this is a simple build + restart.

echo "Building ur-hostd..."
cargo build -p ur-hostd

PID_FILE="${UR_CONFIG:-$HOME/.ur}/hostd.pid"

# Stop existing hostd if running
if [ -f "$PID_FILE" ]; then
    PID=$(cat "$PID_FILE")
    if kill -0 "$PID" 2>/dev/null; then
        echo "Stopping ur-hostd (pid $PID)..."
        kill "$PID"
        sleep 0.5
    fi
    rm -f "$PID_FILE"
fi

# Start via ur start (which handles PID tracking)
echo "Starting ur-hostd..."
target/debug/ur-hostd --port "${UR_HOSTD_PORT:-42070}" &
HOSTD_PID=$!
echo "$HOSTD_PID" > "$PID_FILE"
echo "ur-hostd restarted (pid $HOSTD_PID)"
