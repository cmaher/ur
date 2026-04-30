#!/usr/bin/env bash
# Copy skills from ./skills/ into ~/.claude/skills/.
set -euo pipefail

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/skills"
DEST_DIR="${HOME}/.claude/skills"

mkdir -p "${DEST_DIR}"

for skill in "${SRC_DIR}"/*/; do
    name="$(basename "${skill}")"
    dest="${DEST_DIR}/${name}"
    rm -rf "${dest}"
    cp -R "${skill%/}" "${dest}"
    echo "installed ${dest}"
done
