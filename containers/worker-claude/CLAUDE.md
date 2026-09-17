# worker-claude (Container Image)

Claude-specific layer on top of `ur-worker-base:latest`. Must work with Docker and nerdctl (containerd) runtimes.

- Build context is `containers/worker-claude/` — all files copied into the image must live here
- Image is tagged `ur-worker-claude:latest` by convention — the directory name matches the tag
- `vendor/claude/install.sh` is a local wrapper around the upstream installer — cached in the build context so the Dockerfile doesn't depend on a remote URL directly
- Claude Code is installed into **this** image (agent-specific, not the base) and refreshed by a `claude update` layer gated on the `CACHEBUST` build arg. A plain `cargo make install` leaves that layer cached, so the version never moves — use `cargo make install-update-claude` (busts only this layer) or `cargo make install-nocache` (also rebuilds the base with `--no-cache`). The update is **not** best-effort: a failed `claude update` fails the build rather than silently shipping a stale version
- Entrypoint runs `exec workerd`, making workerd PID 1 — it owns the full container lifecycle (init, tmux, claude, gRPC server)
- Worker command binaries (`ur-ping`, `workertools`, `workerd`) are cross-compiled and staged into `bin/` by `stage-workercmd.sh`, then copied into the image at `/usr/local/bin/`
- `workerd` handles initialization (skills, git hooks, hostexec shims), creates the tmux session, launches Claude Code, and serves gRPC. It reads agent-agnostic content from `/home/worker/.agent-shared/` (baked by `worker-base`) and writes it into Claude's own layout under `~/.claude/`
- `claude.json` (baked to `~/.claude.json`) and `claude-settings.json` (baked to `~/.claude/potential-settings.json`) are Claude-specific app config, not shared content — they stay here even though `~/.claude/skills/` and the transcript directory pre-creation also happen in this image
- `workertools` provides the `host-exec` subcommand used by shims to forward commands to the server via gRPC
- Workers reach the Squid forward proxy at `ur-squid:3128` via Docker DNS; `HTTP_PROXY`/`HTTPS_PROXY` env vars are set by the server at launch

## Running `gh` and `git` (host-exec)

`gh` and `git` are shims that forward to the server and execute on the **host**, not inside your container. The consequence that causes failed retries:

- **The host cannot read files from your container/sandbox filesystem.** Flags that take a file path (notably `gh ... --body-file <path>` and `gh api --input <path>`) fail with "no such file or directory" because that path only exists inside your container.

**stdin forwarding is per-command.** `gh` is a bidi command, so stdin *is* forwarded and the `-` form of file flags works. `git` is not bidi — do not pipe into `git`.

**To post a PR/issue comment or a PR body**, pass the text inline or pipe it:

```bash
gh pr comment <pr> --body "$(cat body.md)"
gh pr create --title "..." --body "$(cat body.md)"
gh pr edit <pr> --body "$(cat body.md)"
gh pr comment <pr> --body-file - < body.md
```

Do **not** use `--body-file <path>` — the shim rejects a path for `gh pr` and points you at inline `--body` or the `-` form.

### PR review and inline comments

Submit a review body as a comment-only review:

```bash
gh pr review <pr> --comment --body "$(cat review.md)"
```

Raw `POST` requests to `/repos/<owner>/<repo>/pulls/<n>/reviews` are blocked because their JSON event could approve a PR or request changes. Post each inline comment through the individual comment endpoint:

```bash
gh api /repos/<owner>/<repo>/pulls/<n>/comments -X POST \
  -f commit_id=<sha> -f path=a.go -F line=42 -f side=RIGHT -f body="..."
```

Existing conversation and inline comments may be updated with `PATCH`:

```bash
gh api /repos/<owner>/<repo>/issues/comments/<comment-id> -X PATCH -f body="..."
gh api /repos/<owner>/<repo>/pulls/comments/<comment-id> -X PATCH -f body="..."
```

Endpoints may be written with or without the leading slash, and as full `https://api.github.com/...` URLs.

Allowed `gh`: `pr view|checks|list|status|diff|comment|edit|create`, `pr review --comment`, `run view|list`, and `gh api` (GET always; POST creates comments or replies; PATCH updates comments). Destructive ops (`pr merge|close|delete`) and review decisions (`--approve`, `--request-changes`, or raw review creation) are blocked. `git` blocks `worktree`, `checkout`/`switch` (use `git restore`), `--no-verify`, `--git-dir`/`--work-tree`, and pushes to branches other than your own. Any blocked command returns a message listing exactly what is allowed.
