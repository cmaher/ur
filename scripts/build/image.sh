#!/usr/bin/env bash
set -euo pipefail

# Build all container images using Docker (or nerdctl).
# Builds:
#   ur-worker-base:<tag> (slow, cached) + ur-worker-<agent>:<tag> (fast) on top
#   ur-server:<tag> (Alpine + cross-compiled ur-server binary)
#
# Set UR_IMAGE_TAG to override the default tag (latest).

tag="${UR_IMAGE_TAG:-latest}"

build_image() {
    local tag="$1"
    local dockerfile="$2"
    local context="$3"
    echo "Building $tag..."
    if command -v docker >/dev/null 2>&1; then
        docker build -t "$tag" -f "$dockerfile" "$context"
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl build -t "$tag" -f "$dockerfile" "$context"
    else
        echo "Warning: no container runtime found, skipping image build" >&2
        exit 1
    fi
}

build_image_arg() {
    local tag="$1"
    local dockerfile="$2"
    local context="$3"
    shift 3
    local args=()
    for arg in "$@"; do
        args+=(--build-arg "$arg")
    done
    echo "Building $tag..."
    if command -v docker >/dev/null 2>&1; then
        docker build "${args[@]}" -t "$tag" -f "$dockerfile" "$context"
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl build "${args[@]}" -t "$tag" -f "$dockerfile" "$context"
    else
        echo "Warning: no container runtime found, skipping image build" >&2
        exit 1
    fi
}

build_image_no_cache() {
    local tag="$1"
    local dockerfile="$2"
    local context="$3"
    echo "Building $tag (no cache)..."
    if command -v docker >/dev/null 2>&1; then
        docker build --no-cache -t "$tag" -f "$dockerfile" "$context"
    elif command -v nerdctl >/dev/null 2>&1; then
        nerdctl build --no-cache -t "$tag" -f "$dockerfile" "$context"
    else
        echo "Warning: no container runtime found, skipping image build" >&2
        exit 1
    fi
}

# True if `agent` should get a fresh CACHEBUST for its update layer this run.
# UR_FORCE_REBUILD_BASE=1 busts every agent (the base rebuild already
# invalidates their cache, so this just makes the intent explicit).
# UR_UPDATE_AGENT names one agent, or a comma-separated list, to bust
# specifically. UR_UPDATE_CLAUDE=1 is a back-compat alias for
# UR_UPDATE_AGENT=claude, kept so `cargo make install-update-claude` needs no
# changes.
should_bust_agent() {
    local agent="$1"
    if [ "${UR_FORCE_REBUILD_BASE:-}" = "1" ]; then
        return 0
    fi
    local update_agents="${UR_UPDATE_AGENT:-}"
    if [ "${UR_UPDATE_CLAUDE:-}" = "1" ]; then
        update_agents="${update_agents:+$update_agents,}claude"
    fi
    case ",${update_agents}," in
        *",${agent},"*) return 0 ;;
        *) return 1 ;;
    esac
}

BASE_CONTEXT=containers/worker-base

# Directory name matches image tag name exactly, so this table maps 1:1 onto
# `containers/<dir>` -> `<tag>:<image-tag>`. Adding an agent's images (e.g.
# codex) is a data change here, not a code change to the loop below.
#
# Fields: context dir : image tag (no version suffix) : agent name (for
# UR_UPDATE_AGENT matching) : whether this layer has its own CACHEBUST-gated
# agent-CLI-update step (only the base agent layer does — a variant built on
# top of it, like the rust toolchain image, picks up the parent's new content
# automatically once Docker sees the parent image ID changed).
AGENT_IMAGES=(
    "worker-claude:ur-worker-claude:claude:true"
    "worker-rust-claude:ur-worker-rust-claude:claude:false"
)

# Stage vendored mise installer into the rust worker build context
cp "$BASE_CONTEXT/vendor/mise/install.sh" "containers/worker-rust-claude/install-mise.sh"

# `ur-worker-base` now carries only agent-agnostic assets (shell setup, skills,
# instructions) — no agent CLI install — so UR_FORCE_REBUILD_BASE no longer
# touches any agent CLI install at all. It still fully rebuilds this layer.
if [ "${UR_FORCE_REBUILD_BASE:-}" = "1" ]; then
    build_image_no_cache "ur-worker-base:$tag" "$BASE_CONTEXT/Dockerfile" "$BASE_CONTEXT"
else
    build_image "ur-worker-base:$tag" "$BASE_CONTEXT/Dockerfile" "$BASE_CONTEXT"
fi
echo "Base image built: ur-worker-base:$tag"

# Each agent CLI install (and its own `<agent> update` layer, where it has
# one) lives in its own image, not the base. That layer is cached like any
# other, so the agent stays pinned at the version baked in until CACHEBUST
# changes. Both UR_FORCE_REBUILD_BASE=1 (full base rebuild, which invalidates
# this layer's cache since it's FROM the base) and UR_UPDATE_AGENT=<name>
# (cheap: just this layer) bust it. The update is not best-effort: a failed
# `claude update` fails the build rather than silently shipping a stale
# version.
for entry in "${AGENT_IMAGES[@]}"; do
    IFS=':' read -r dir img_tag agent_name needs_cachebust <<< "$entry"
    context="containers/$dir"
    args=("BASE_TAG=$tag")
    if [ "$needs_cachebust" = "true" ] && should_bust_agent "$agent_name"; then
        args+=("CACHEBUST=$(date +%s)")
    fi
    build_image_arg "$img_tag:$tag" "$context/Dockerfile" "$context" "${args[@]}"
    echo "Image built: $img_tag:$tag"
done

build_image "ur-server:$tag" containers/server/Dockerfile containers/server
echo "ur-server image built: ur-server:$tag"

build_image "ur-squid:$tag" containers/squid/Dockerfile containers/squid
echo "Squid proxy image built: ur-squid:$tag"
