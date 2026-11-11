#!/usr/bin/env bash
set -euo pipefail

# Install the agent assets local workers read (builderd --agent-shared) into
# DEST, replacing any previous copy. Same order and override as the
# `COPY --from=shared` steps in containers/worker-<agent>/Dockerfile: the
# repo's skills land after the vendored
# ones and win a name collision. Used by install.sh and the acceptance suite.
#
#   scripts/build/install-agent-assets.sh <dest>

DEST="${1:?usage: install-agent-assets.sh <dest>}"
BASE="$(cd "$(dirname "$0")/../.." && pwd)/containers/worker-base"

rm -rf "$DEST"
mkdir -p "$DEST/potential-skills"
cp -R "$BASE/vendor/superpowers/skills/." "$DEST/potential-skills/"
cp -R "$BASE/potential-skills/." "$DEST/potential-skills/"
cp -R "$BASE/instructions" "$DEST/instructions"
cp -R "$BASE/shared-instructions" "$DEST/shared-instructions"
