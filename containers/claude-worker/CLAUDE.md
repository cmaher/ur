# claude-worker (Container Image)

Debian bookworm-slim container image for agent workers. Must work with Docker and nerdctl (containerd) runtimes.

- Build context is `containers/claude-worker/` — all files copied into the image must live here
- Image is tagged `ur-worker:latest` by convention
- `install-claude.sh` is a local wrapper around the upstream installer — cached in the build context so the Dockerfile doesn't depend on a remote URL directly
- Entrypoint uses bash (not sh) and starts a tmux session named `agent` to keep the container alive
- Worker command binaries (`ur-ping`, `workertools`, `workerd`) are cross-compiled and copied into the image at `/usr/local/bin/`
- `workerd` runs as a background daemon (started by entrypoint.sh) that creates command shims in `~/.local/bin/` for host-executed commands (e.g., git, gh)
- `workertools` provides the `host-exec` subcommand used by shims to forward commands to the server via gRPC
- Workers reach the Squid forward proxy at `ur-squid:3128` via Docker DNS; `HTTP_PROXY`/`HTTPS_PROXY` env vars are set by the server at launch
