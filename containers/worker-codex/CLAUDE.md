# worker-codex (Container Image)

Codex-specific layer on top of `ur-worker-base:latest`, mirroring `containers/worker-claude/`
layer for layer. Must work with Docker and nerdctl (containerd) runtimes.

- Build context is `containers/worker-codex/` — all files copied into the image must live here
- Image is tagged `ur-worker-codex:latest` by convention — the directory name matches the tag
- `vendor/codex/install.sh` is a wrapper we wrote (not a vendored upstream script — codex has no
  fixed-checksum manifest to pin against the way Claude Code's GCS bucket does) that resolves
  the current release from GitHub's API and installs the native
  `codex-{aarch64,x86_64}-unknown-linux-musl` binary at `/usr/local/bin/codex`. No node in this
  image — codex ships a single native binary, unlike the npm-distributed Claude CLI
- Codex is installed as **root** (a system-wide binary, not a per-user install), then
  `chown worker:worker /usr/local/bin/codex` so the later `codex update` layer (run as `worker`)
  can overwrite it
- Refreshed by re-running `install.sh` in a layer gated on the `CACHEBUST` build arg — use
  `UR_UPDATE_AGENT=codex` (see `scripts/build/image.sh`) to bust only this layer. This is
  **not** `codex update`: that subcommand refuses to touch a binary it didn't install itself
  (`Could not detect the Codex installation method`), unlike Claude Code's CLI, which tracks
  and self-updates a manually-dropped native binary fine. Re-running the install script is the
  actual update mechanism here, so `install-codex.sh` is kept around in the image (not deleted
  until after this layer) specifically so it can run a second time. The update is **not**
  best-effort: a failed re-install fails the build rather than silently shipping a stale version

## The `potential-settings.json`-holding-TOML wrinkle

`InitSettingsManager` (`crates/workerd/src/init/settings.rs`) copies
`~/{agent.home_subdir()}/potential-settings.json` to `~/{agent.home_subdir()}/{settings_filename()}`
as a **byte-verbatim copy** — it never parses the source, so the source's actual format doesn't
matter, only its name. For Claude that name coincidentally matches its content (JSON). For codex,
`settings_filename()` is `"config.toml"`, but the *source* the Dockerfile bakes is still named
`potential-settings.json` (`codex-config.toml` in this build context, copied to
`~/.codex/potential-settings.json`) even though its content is TOML — renaming the hardcoded
source filename to be agent-aware wasn't needed to ship codex support, so it stayed as-is.
Don't "fix" the extension without also generalizing `InitSettingsManager`'s source path.

## Managed hooks, not user config

The epic's spike (see `ur-5a234`'s epic body) found that `bypass_hook_trust` is **not** a
`config.toml` key — it exists only as the `--dangerously-bypass-hook-trust` CLI flag. A hook
declared in the user config would sit behind codex's interactive hook-trust gate, which never
clears in a non-interactive container. `managed_config.toml`, baked to `/etc/codex/managed_config.toml`
as root (a path only root can write — we own `/etc` inside the container), sidesteps this
entirely: the TUI reports "Managed hooks are always on." `allow_managed_hooks_only = true`
additionally blocks a checked-out repo from shipping its own hooks into the worker — the right
posture for a container running other people's code.

**Fallback, not the plan:** if managed hooks ever turn out to still require trust, pass
`--dangerously-bypass-hook-trust` from `AgentType::Codex.spawn_command()` instead. That's
strictly worse — it trusts *any* hook a checked-out repo ships — so reach for it only if the
managed layer stops working, and record why here if you do.

**An unrecognized hook event name is silently accepted, not rejected.** Codex does not validate
event names in `[hooks]` against a known set — typo `SesionStart` or `Stopp` and the config
still loads cleanly, the event key just never matches anything and the hook never fires. There
is no config-time signal that a hook is wired wrong. If you add or rename a hook here, verify it
by actually triggering the event (launch a worker and confirm `workertools notify-idle` ran —
e.g. check that the worker reports `idle` via `ur worker list`, or check server/worker logs for
the `NotifyIdle` RPC) rather than trusting `codex` to complain about a bad event name.

## Other config

- `approval_policy = "never"` and `sandbox_mode = "danger-full-access"` in the baked config:
  the container itself is the sandbox boundary (network via `ur-squid`, filesystem via the
  `/workspace` mount), so codex's own approval/sandbox layer would only add prompts nothing in
  this non-interactive setup can answer — the same reasoning as Claude's
  `permissions.defaultMode: "bypassPermissions"`
- `project_doc_fallback_filenames = ["CLAUDE.md"]` lets codex read a repo's existing `CLAUDE.md`
  when it has no `AGENTS.md` — no ur-managed repo needs to add one just for codex
- `[projects."/workspace"] trust_level = "trusted"` skips the interactive trust prompt for the
  one path every worker actually mounts
- `model_reasoning_effort = "high"` is a starting default for an autonomous coding agent, not a
  verified-optimal value — revisit if cost or latency becomes a concern
- Entrypoint runs `exec workerd`, making workerd PID 1 — same as `worker-claude`
- Worker command binaries (`ur-ping`, `workertools`, `workerd`) are staged into `bin/` by
  `stage-workercmd.sh`, then copied into the image at `/usr/local/bin/`
- `workerd` handles initialization (skills, git hooks, hostexec shims), creates the tmux
  session, launches codex, and serves gRPC — reads agent-agnostic content from
  `/home/worker/.agent-shared/` (baked by `worker-base`) and writes it into codex's own layout
  under `~/.codex/`, same as for Claude
- Workers reach the Squid forward proxy at `ur-squid:3128` via Docker DNS;
  `HTTP_PROXY`/`HTTPS_PROXY` env vars are set by the server at launch
