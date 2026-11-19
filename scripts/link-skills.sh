#!/usr/bin/env bash
# Install host skills into ~/.claude/skills/:
#   - every skill in ./skills/
#   - the brain skills from containers/worker-base/potential-skills/, under their bare
#     names so their cross-references (/brain:init, /brain:dream-init, ...) resolve.
#   - the host-usable worker skills auto, trim, drive-workers and code-review from the same
#     directory.
# Workers get the same single copy installed by workerd.
#
# A skill is symlinked unless it uses the %UR_SKILL_HOOKS% placeholder. Those get a rendered
# copy instead, with the placeholder pointing at UR_HOST_SKILL_HOOKS (default
# ~/.claude/skill-hooks), the host's equivalent of a worker's skill-hooks directory. Re-run
# this script after pulling to refresh rendered copies.
set -euo pipefail

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
POTENTIAL_DIR="${REPO_DIR}/containers/worker-base/potential-skills"
DEST_DIR="${HOME}/.claude/skills"
HOST_SKILL_HOOKS="${UR_HOST_SKILL_HOOKS:-${HOME}/.claude/skill-hooks}"
PLACEHOLDER='%UR_SKILL_HOOKS%'

mkdir -p "${DEST_DIR}" "${HOST_SKILL_HOOKS}"

install_skill() {
    local src="${1%/}"
    local dest="${DEST_DIR}/$(basename "${src}")"
    rm -rf "${dest}"
    if grep -rqF "${PLACEHOLDER}" "${src}"; then
        cp -R "${src}" "${dest}"
        find "${dest}" -type f -exec perl -pi -e "s|\Q${PLACEHOLDER}\E|${HOST_SKILL_HOOKS}|g" {} +
        echo "rendered ${dest} (skill hooks: ${HOST_SKILL_HOOKS})"
    else
        ln -s "${src}" "${dest}"
        echo "linked ${dest} -> ${src}"
    fi
}

for skill in "${REPO_DIR}"/skills/*/; do
    install_skill "${skill}"
done

for skill in "${POTENTIAL_DIR}"/brain/ "${POTENTIAL_DIR}"/brain:*/ \
    "${POTENTIAL_DIR}"/auto/ "${POTENTIAL_DIR}"/trim/ "${POTENTIAL_DIR}"/drive-workers/ \
    "${POTENTIAL_DIR}"/code-review/; do
    install_skill "${skill}"
done
