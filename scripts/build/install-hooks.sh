#!/usr/bin/env bash
set -euo pipefail

# Install ur's git hooks (pre-commit formats, pre-push runs CI + acceptance)
# into this repository's hooks directory. `git rev-parse --git-path hooks`
# resolves it correctly from a linked worktree (where .git is a file) and
# honours core.hooksPath.

cd "$(dirname "$0")/../.."
hooks=$(git rev-parse --git-path hooks)
mkdir -p "$hooks"
cp ur-hooks/git/* "$hooks/"
echo "Installed git hooks into $hooks"
