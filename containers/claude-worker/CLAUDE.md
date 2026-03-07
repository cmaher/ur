# claude-worker (Container Image)

Debian bookworm-slim container image for agent workers. Must work with Apple (`container`), Docker, and nerdctl (containerd) runtimes.

- Build context is `containers/claude-worker/` — all files copied into the image must live here
- Image is tagged `ur-worker:latest` by convention
- `install-claude.sh` is a local wrapper around the upstream installer — cached in the build context so the Dockerfile doesn't depend on a remote URL directly
- Entrypoint uses bash (not sh) and starts a tmux session named `agent` to keep the container alive
- `agent_tools` binary will be copied in at `/usr/local/bin/` once cross-compilation is wired up
