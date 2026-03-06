# claude-worker (Container Image)

Container image for agent workers. Must work with both Apple (`container`) and Docker runtimes.

- Build context is `containers/claude-worker/` — all files copied into the image must live here
- Image is tagged `ur-worker:latest` by convention
- Entrypoint starts tmux session named `agent` to keep the container alive
- `agent_tools` binary will be copied in at `/usr/local/bin/` once cross-compilation is wired up
