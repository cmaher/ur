# claude-worker (Container Image)

Debian bookworm-slim container image for agent workers. Must work with Apple (`container`), Docker, and nerdctl (containerd) runtimes.

- Build context is `containers/claude-worker/` — all files copied into the image must live here
- Image is tagged `ur-worker:latest` by convention
- `install-claude.sh` is a local wrapper around the upstream installer — cached in the build context so the Dockerfile doesn't depend on a remote URL directly
- Entrypoint uses bash (not sh) and starts a tmux session named `agent` to keep the container alive
- Worker command binaries (`ur-ping`, `git`, `gh`) are cross-compiled and copied into the image at `/usr/local/bin/`
- `git` binary is a transparent proxy that forwards all git commands to the server's GitService via gRPC over TCP (`$UR_SERVER_ADDR`)
- `gh` binary is a transparent proxy that forwards all gh commands to the server's GhService via gRPC over TCP (`$UR_SERVER_ADDR`)
- Workers reach the Squid forward proxy at `ur-squid:3128` via Docker DNS; `HTTP_PROXY`/`HTTPS_PROXY` env vars are set by the server at launch
