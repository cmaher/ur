#!/usr/bin/env bash

# Single source of truth for per-agent worker image contexts.
# Fields: context dir : image tag : agent : has CACHEBUST layer : stage worker binaries
AGENT_IMAGES=(
    "worker-claude:ur-worker-claude:claude:true:true"
    "worker-rust-claude:ur-worker-rust-claude:claude:false:false"
    "worker-codex:ur-worker-codex:codex:true:true"
    "worker-rust-codex:ur-worker-rust-codex:codex:false:false"
    "worker-agy:ur-worker-agy:agy:true:true"
    "worker-rust-agy:ur-worker-rust-agy:agy:false:false"
)
