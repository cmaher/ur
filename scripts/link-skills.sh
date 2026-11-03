#!/usr/bin/env bash
# Symlink host skills into ~/.claude/skills/:
#   - every skill in ./skills/
#   - the brain skills from containers/worker-base/potential-skills/, under their bare
#     names so their cross-references (/brain:init, /brain:dream-init, ...) resolve.
#     Workers get the same single copy installed by workerd.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POTENTIAL_DIR="${REPO_DIR}/containers/worker-base/potential-skills"
DEST_DIR="${HOME}/.claude/skills"

mkdir -p "${DEST_DIR}"

link() {
    local src="${1%/}"
    local dest="${DEST_DIR}/$(basename "${src}")"
    rm -rf "${dest}"
    ln -s "${src}" "${dest}"
    echo "linked ${dest} -> ${src}"
}

for skill in "${REPO_DIR}"/skills/*/; do
    link "${skill}"
done

for skill in "${POTENTIAL_DIR}"/brain/ "${POTENTIAL_DIR}"/brain:*/; do
    link "${skill}"
done
