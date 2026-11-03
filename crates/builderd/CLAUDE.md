# builderd

Builder daemon for executing commands and managing containers on the host. Receives
already-validated requests from ur-server and acts on the host filesystem and Docker daemon.

- Standalone binary, runs natively on host (not containerized)
- tonic gRPC server; binds to `127.0.0.1` by default, `--bind` flag overrides (on Linux, `ur start` passes the Docker bridge gateway IP)
- Trusts ur-server completely — no command validation
- Started/stopped by `ur start`/`ur stop`, PID tracked at `~/.ur/builderd.pid`
- Resolves `%WORKSPACE%` prefixes in both `command` and `working_dir` via `--workspace` CLI flag or `BUILDERD_WORKSPACE` env var
- Exposes three gRPC services on the same port:
  - `BuilderDaemonService` (`proto/builder.proto`) — exec arbitrary commands on the host; used for worker hostexec (git, gh via the three-hop pipeline)
  - `BuilderContainerService` (`proto/builder_container.proto`) — worker container lifecycle (launch, stop, exec, network inspect); owns the Docker socket on behalf of the server
  - `BuilderPoolService` (`proto/builder_pool.proto`) — pool slot lifecycle (clone, reset, clean, branch checkout); owns all pool filesystem and git operations on behalf of `RepoPoolManager`
- `LaunchWorker` does not report success immediately after `docker run`: it waits up to 60 seconds for the worker image's health check. An `unhealthy` result remains pending because synchronous startup hooks can outlast Docker's health-check start period. If the container exits, builderd captures its last 100 log lines, removes it, and returns `FailedPrecondition`.
- `--config-dir` flag (default `~/.ur`) sets the root config directory; used by `LocalOverlayManager` to locate local overlay files at `<config_dir>/projects/<project>/local/`
- Local overlays (`overlay.rs`, `LocalOverlayManager`) are installed into pool slots on acquire and kept under `<slot>/.git/ur-overlay/`. Every short-lived `git`/`gh` exec in an overlaid slot is bracketed (conceal → run → reveal before the exit code frame) so git never sees overlays. Any new builderd path that runs git in a slot on a worker's behalf must go through the same bracket. See `docs/codeflows/project-file-mounting.md#hiding-overlays-from-git`
- Local workers (`local_workspace.rs`, `LocalWorkspaceManager`; `local_settings.rs`, `LocalSettingsManager`) build `<workspace_root>/drive/<worker_id>/`: mount/brain symlinks, the slot's Claude memory symlink, and `.ur/{settings.json, instructions.md, plugin/, skill-hooks/}` via the shared workerd init managers (`SkillLayout::ur_plugin()`), plus git hooks into the slot; `build()` returns the `claude` argv. Data-safety rules: teardown removes only symlinks and the drive (`remove_dir_all` never follows links), never a link target; build never clobbers a real non-empty memory dir and refuses mount dests that would nest through another mount's link. Project instructions are inlined (not `@`-imported) because `--append-system-prompt-file` does not expand imports. Not yet wired into a gRPC service (ur-4p34c).
