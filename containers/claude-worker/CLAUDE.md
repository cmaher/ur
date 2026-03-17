# claude-worker (Container Image)

Debian bookworm-slim container image for agent workers. Must work with Docker and nerdctl (containerd) runtimes.

- Build context is `containers/claude-worker/` — all files copied into the image must live here
- Image is tagged `ur-worker:latest` by convention
- `install-claude.sh` is a local wrapper around the upstream installer — cached in the build context so the Dockerfile doesn't depend on a remote URL directly
- Entrypoint runs `exec workerd`, making workerd PID 1 — it owns the full container lifecycle (init, tmux, claude, gRPC server)
- Worker command binaries (`ur-ping`, `workertools`, `workerd`) are cross-compiled and copied into the image at `/usr/local/bin/`
- `workerd` handles initialization (skills, git hooks, hostexec shims), creates the tmux session, launches Claude Code, and serves gRPC
- `workertools` provides the `host-exec` subcommand used by shims to forward commands to the server via gRPC
- Workers reach the Squid forward proxy at `ur-squid:3128` via Docker DNS; `HTTP_PROXY`/`HTTPS_PROXY` env vars are set by the server at launch
