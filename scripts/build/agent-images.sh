#!/usr/bin/env bash

# Single source of truth for per-agent worker image contexts.
# Fields: context dir : image tag : agent : has CACHEBUST layer : stage worker binaries
AGENT_IMAGES=(
    "worker-claude:ur-worker-claude:claude:true:true"
    "worker-codex:ur-worker-codex:codex:true:true"
    "worker-agy:ur-worker-agy:agy:true:true"
)
