#!/bin/bash
set -e

mkdir -p ~/.claude
mkdir -p ~/.local/bin

# Initialize: skills, git hooks, hostexec shims (synchronous)
workerd init

# Sweep stale cargo build artifacts in the background (best-effort).
# target/ persists across workers in pool slots, so old artifacts accumulate.
(cd /workspace && [ -f Cargo.toml ] && cargo sweep --time 1 &) 2>/dev/null

# Start bacon on the host via hostexec shim.
# Uses the 'ai' job (cargo check --message-format short) and exports
# diagnostics to .bacon-locations for agents to read.
(cd /workspace && bacon --headless ai &)

# Start workerd daemon (skipping init — already done above).
# exec makes workerd PID 1 — container lifecycle is tied to it.
exec workerd daemon
