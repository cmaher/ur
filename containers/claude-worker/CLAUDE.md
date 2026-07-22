# claude-worker (Container Image)

Debian bookworm-slim container image for agent workers. Must work with Docker and nerdctl (containerd) runtimes.

- Build context is `containers/claude-worker/` — all files copied into the image must live here
- Image is tagged `ur-worker:latest` by convention
- `install-claude.sh` is a local wrapper around the upstream installer — cached in the build context so the Dockerfile doesn't depend on a remote URL directly
- Entrypoint runs `exec workerd`, making workerd PID 1 — it owns the full container lifecycle (init, tmux, claude, gRPC server)
- Worker command binaries (`ur-ping`, `workertools`, `workerd`) are cross-compiled and staged into `bin/` by `stage-workercmd.sh`, then copied into the image at `/usr/local/bin/`
- `workerd` handles initialization (skills, git hooks, hostexec shims), creates the tmux session, launches Claude Code, and serves gRPC
- `workertools` provides the `host-exec` subcommand used by shims to forward commands to the server via gRPC
- Workers reach the Squid forward proxy at `ur-squid:3128` via Docker DNS; `HTTP_PROXY`/`HTTPS_PROXY` env vars are set by the server at launch

## Running `gh` and `git` (host-exec)

`gh` and `git` are shims that forward to the server and execute on the **host**, not inside your container. Two consequences that cause failed retries if you don't account for them:

- **The host cannot read files from your container/sandbox filesystem.** Flags that take a file path (notably `gh ... --body-file <path>`) will fail with "no such file or directory" because that path only exists inside your container.
- **stdin is not forwarded.** Piping a heredoc or using `--body-file -` fails with "Body cannot be blank".

**To post a PR/issue comment or a PR body, pass the text inline** so your shell expands it locally and the content is forwarded as an argument:

```bash
gh pr comment <pr> --body "$(cat body.md)"
gh pr create --title "..." --body "$(cat body.md)"
gh pr edit <pr> --body "$(cat body.md)"
```

Do **not** use `--body-file <path>`, `--body-file -`, or `| gh ...` — the shim rejects `--body-file` for `gh pr` with a reminder to use inline `--body`.

Allowed `gh`: `pr view|checks|list|status|diff|comment|edit|create`, `pr review --comment`, `run view|list`, and `gh api` (GET always; POST/PATCH only to comment/review endpoints). Destructive ops (`pr merge|close|delete`) are workflow-only and blocked. `git` blocks `worktree`, `checkout`/`switch` (use `git restore`), `--no-verify`, `--git-dir`/`--work-tree`, and pushes to branches other than your own. Any blocked command returns a message listing exactly what is allowed.
