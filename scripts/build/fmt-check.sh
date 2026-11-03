#!/usr/bin/env bash
# Auto-fix formatting, then fail if changes were needed.
set -euo pipefail

if ! cargo fmt --all --check >/dev/null 2>&1; then
    cargo fmt --all >/dev/null 2>&1
    echo "error: formatting was incorrect. Files are already fixed on disk. Stage, commit, and push again."
    exit 1
fi
