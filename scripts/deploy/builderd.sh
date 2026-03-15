#!/usr/bin/env bash
set -euo pipefail

# Rebuild and restart builderd on the host.
# builderd runs natively (not in a container), so this is a simple build + restart.

echo "Building builderd..."
cargo build -p builderd

PID_FILE="${UR_CONFIG:-$HOME/.ur}/builderd.pid"

# Stop existing builderd if running
if [ -f "$PID_FILE" ]; then
    PID=$(cat "$PID_FILE")
    if kill -0 "$PID" 2>/dev/null; then
        echo "Stopping builderd (pid $PID)..."
        kill "$PID"
        sleep 0.5
    fi
    rm -f "$PID_FILE"
fi

# Start via ur start (which handles PID tracking)
echo "Starting builderd..."
target/debug/builderd --port "${UR_BUILDERD_PORT:-42070}" &
BUILDERD_PID=$!
echo "$BUILDERD_PID" > "$PID_FILE"
echo "builderd restarted (pid $BUILDERD_PID)"
