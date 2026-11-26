#!/usr/bin/env bash
# Symlink local skills (skills/local/* and skills/ur:*) into host agent skill
# directories (Claude Code, AGY, and Codex) and prune stale repo-managed symlinks.
#
# Usage:
#   scripts/link-skills.sh [--dry-run] [--no-check] [--force]
#
# Options:
#   --dry-run   Print planned changes without modifying the filesystem
#   --no-check  Skip running scripts/check-skills.sh first
#   --force     Replace colliding non-symlink directories or external symlinks
#
# Target directory overrides (for testing or custom installs):
#   CLAUDE_SKILLS_DIR  (default: ~/.claude/skills)
#   AGY_SKILLS_DIR     (default: ~/.gemini/config/skills)
#   CODEX_SKILLS_DIR   (default: ${CODEX_HOME:-~/.codex}/skills)
set -euo pipefail
shopt -s nullglob

DRY_RUN=0
RUN_CHECK=1
FORCE=0

for arg in "$@"; do
    case "$arg" in
        --dry-run)  DRY_RUN=1 ;;
        --no-check) RUN_CHECK=0 ;;
        --force)    FORCE=1 ;;
        -h|--help)
            sed -n '2,16p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "error: unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd -P)"
LOCAL_DIR="${REPO_DIR}/skills/local"

if [ "${RUN_CHECK}" -eq 1 ]; then
    "${REPO_DIR}/scripts/check-skills.sh"
fi

SOURCES=()
for src in "${LOCAL_DIR}"/*/ "${REPO_DIR}"/skills/ur:*/; do
    src="${src%/}"
    [ -f "${src}/SKILL.md" ] || continue
    SOURCES+=("${src}")
done

if [ "${#SOURCES[@]}" -eq 0 ]; then
    echo "error: no local skills found under ${LOCAL_DIR} or ${REPO_DIR}/skills/ur:*" >&2
    exit 1
fi

is_desired() {
    local needle="$1" src
    for src in "${SOURCES[@]}"; do
        [ "${src##*/}" = "$needle" ] && return 0
    done
    return 1
}

is_repo_managed_target() {
    case "$1" in
        "${REPO_DIR}/skills/"*|"${REPO_DIR}/containers/worker-base/potential-skills/"*)
            return 0
            ;;
        *)
            return 1
            ;;
    esac
}

run_action() {
    local dry_verb="$1" done_verb="$2" detail="$3"
    shift 3
    if [ "${DRY_RUN}" -eq 1 ]; then
        echo "[dry-run] ${dry_verb} ${detail}"
    else
        "$@"
        echo "${done_verb} ${detail}"
    fi
}

replace_dir() { rm -rf "$1" && ln -s "$2" "$1"; }

# Skills previously installed by older scripts that are no longer linked locally.
RETIRED_COPIES=(
    ship green reclaude dispatch bacon skill-hooks
    implement-agents brain:dream-init drive-workers
)

RAW_DEST_DIRS=(
    "${CLAUDE_SKILLS_DIR:-${HOME}/.claude/skills}"
    "${AGY_SKILLS_DIR:-${HOME}/.gemini/config/skills}"
    "${CODEX_SKILLS_DIR:-${CODEX_HOME:-${HOME}/.codex}/skills}"
)

DEST_DIRS=()
for raw_dir in "${RAW_DEST_DIRS[@]}"; do
    while [ "${raw_dir}" != "/" ] && [ "${raw_dir%/}" != "${raw_dir}" ]; do
        raw_dir="${raw_dir%/}"
    done
    seen=0
    for existing in "${DEST_DIRS[@]+"${DEST_DIRS[@]}"}"; do
        if [ "${existing}" = "${raw_dir}" ]; then
            seen=1
            break
        fi
    done
    if [ "${seen}" -eq 0 ]; then
        DEST_DIRS+=("${raw_dir}")
    fi
done

CONFLICTS=0

link_into() {
    local dest_dir="$1"
    if [ "${DRY_RUN}" -eq 0 ]; then
        mkdir -p "${dest_dir}"
    fi

    # 1. Prune stale symlinks pointing into repo-managed skill roots only.
    if [ -d "${dest_dir}" ]; then
        local entry target name
        for entry in "${dest_dir}"/*; do
            [ -L "${entry}" ] || continue
            target="$(readlink "${entry}")"
            if is_repo_managed_target "${target}"; then
                name="$(basename "${entry}")"
                if ! is_desired "${name}" || [ ! -e "${entry}" ]; then
                    run_action prune pruned "${entry} -> ${target}" rm "${entry}"
                fi
            fi
        done
    fi

    # 2. Link every desired skill.
    local src dest current
    for src in "${SOURCES[@]}"; do
        dest="${dest_dir}/${src##*/}"

        if [ -L "${dest}" ]; then
            current="$(readlink "${dest}")"
            if [ "${current}" = "${src}" ]; then
                continue
            fi
            if is_repo_managed_target "${current}" || [ "${FORCE}" -eq 1 ]; then
                run_action relink relinked "${dest} -> ${src}" ln -sfn "${src}" "${dest}"
            else
                echo "error: ${dest} is a symlink to external target ${current}; refusing to overwrite (re-run with --force)" >&2
                CONFLICTS=$((CONFLICTS + 1))
            fi
        elif [ -d "${dest}" ]; then
            if [ "${FORCE}" -eq 1 ]; then
                run_action "replace dir" "replaced dir" "${dest} -> ${src}" replace_dir "${dest}" "${src}"
            else
                echo "error: ${dest} is an existing directory; refusing to overwrite (re-run with --force to replace legacy copies)" >&2
                CONFLICTS=$((CONFLICTS + 1))
            fi
        elif [ -e "${dest}" ]; then
            echo "error: ${dest} exists and is not a directory or symlink; refusing to overwrite" >&2
            CONFLICTS=$((CONFLICTS + 1))
        else
            run_action link linked "${dest} -> ${src}" ln -s "${src}" "${dest}"
        fi
    done

    # 3. Warn (never delete) if real legacy directories for retired skills remain.
    local retired legacy
    for retired in "${RETIRED_COPIES[@]}"; do
        legacy="${dest_dir}/${retired}"
        if [ -d "${legacy}" ] && [ ! -L "${legacy}" ]; then
            echo "note: legacy copy ${legacy} is no longer managed by link-skills.sh; remove manually if unwanted" >&2
        fi
    done
}

for dest_dir in "${DEST_DIRS[@]}"; do
    link_into "${dest_dir}"
done

if [ "${CONFLICTS}" -gt 0 ]; then
    echo "link-skills: finished with ${CONFLICTS} conflict(s)" >&2
    exit 1
fi

if [ "${DRY_RUN}" -eq 1 ]; then
    echo "[dry-run] checked ${#SOURCES[@]} skills across ${#DEST_DIRS[@]} agent directories"
else
    echo "linked ${#SOURCES[@]} skills across ${#DEST_DIRS[@]} agent directories"
fi
