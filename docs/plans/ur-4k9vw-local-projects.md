# ur-4k9vw — Repo-less local projects (`local = true`)

## Goal

Let `ur` manage **arbitrary local projects** — a directory on the host with no git remote (or a remote we never clone from) — with the same per-project affordances real projects get: a selected container image, per-project hostexec commands and scripts, mounts, ports, `CLAUDE.md`, `brain_dir` / `memory_dir`, TUI theme.

Interaction model is **workspace mode only**: `umanw` (`ur worker launch -m manual -w . --rm`) from inside the project directory, or explicitly `-w <dir> -p <key>`.

Tickets **can** be created against a local project. Tickets **cannot** be dispatched — there is nothing to clone into a pool slot, no branch to push, no PR to open. Every pool-backed path must refuse a local project loudly and early.

## Why this is small

Most of the machinery already exists and was verified against current `master`:

| Capability | Status |
|---|---|
| `-p <key> -w <dir>` skips the pool entirely and still applies project image/mounts/hostexec/CLAUDE.md/brain/memory | Already works — `crates/server/src/grpc.rs:333-345` ("using workspace dir with project key (no pool slot)") |
| `umanw` alone derives the project key from the cwd directory name | Already works — `resolve_project_key`, `crates/ur/src/main.rs:1288` |
| Ticket creation is gated on the project existing in the registry, nothing more | Already works — `crates/server/src/grpc_ticket.rs:698` |
| `repo` is only consumed by pool clone and PR creation | `crates/server/src/pool.rs:150,189,276`; `crates/server/src/workflow/handlers/push.rs:697` |

The single hard blocker is that `repo` is a **required** field on `RawProjectConfig` (`crates/ur_config/src/lib.rs:468`). The rest of the work is guardrails so a local project fails fast instead of failing halfway through a clone.

## Design decisions (settled)

1. **Explicit opt-in.** `local = true` in `[projects.<key>]`. `repo` stays required unless `local = true`. Supplying both is a config error — this keeps a typo in `repo` a loud failure rather than a silent downgrade to local.
2. **Hard error at dispatch**, in the CLI, the TUI, and server-side.
3. **Pool operations refuse local keys up front** — `RepoPoolManager` rejects before any git work; `ur project add` for a local project creates no pool directory.
4. **TUI marks local-project tickets non-dispatchable** — rendered blocked, dispatch keybinding is a no-op with a banner, no wasted RPC round-trip.

Not in scope (deliberately): an optional `path = "..."` field so `uman <key>` works without `-w .`. Workspace mode is the entry point. Revisit only if the `-w .` ergonomics prove annoying.

## Types and interfaces

### `crates/ur_config/src/lib.rs`

`RawProjectConfig` — `repo` becomes optional, `local` is added:

```rust
struct RawProjectConfig {
    /// Git remote URL. Required unless `local = true`.
    repo: Option<String>,
    /// Declares a repo-less local project: no remote, no pool, no dispatch.
    /// Mutually exclusive with `repo`.
    #[serde(default)]
    local: bool,
    // ...all existing fields unchanged
}
```

`ProjectConfig` — `repo` becomes an `Option`, with a locality accessor. Making it `Option<String>` rather than keeping `String` and adding a bool is the point: every consumer of `repo` is forced by the compiler to state what it does when there is no remote.

```rust
pub struct ProjectConfig {
    pub key: String,
    /// Git remote URL. `None` for a local project (`local = true`).
    pub repo: Option<String>,
    // ...all existing fields unchanged
}

impl ProjectConfig {
    /// True when this project has no git remote — no pool, no dispatch, no PRs.
    pub fn is_local(&self) -> bool;
    /// The git remote, or a descriptive error naming the key and the reason.
    pub fn require_repo(&self) -> anyhow::Result<&str>;
}
```

Validation added to `resolve_project_config` (around `crates/ur_config/src/lib.rs:1548`):

- `local = true` + `repo` present → error naming both fields.
- `local = false`/absent + `repo` absent → the existing "missing repo" error, message extended to mention `local = true`.
- A local project with `pool_limit` set → error (`pool_limit` is meaningless without a remote). Same treatment as the existing "removed field" hard errors.
- `protected_branches`, `max_implement_cycles`, `max_fix_attempts`, `push_again_exit_code`, `ignored_workflow_checks` are all workflow-only. Decide per-field whether to reject or silently ignore for local projects — see Open questions.

### `crates/server/src/project_registry.rs`

```rust
impl ProjectRegistry {
    /// True when the key names a configured local project.
    /// False for unknown keys and for repo-backed projects.
    pub fn is_local(&self, key: &str) -> bool;
}
```

### `crates/server/src/pool.rs`

`RepoPoolManager::acquire_slot`, `acquire_shared_slot`, and `prepare_shared_slot` gain an up-front locality check that returns before any builderd RPC. New error variant on the pool error type:

```rust
/// Attempted a pool operation against a project with no git remote.
LocalProject { project_key: String },
```

Message shape: `project '<key>' is local (local = true, no repo) — pool slots require a git remote; use workspace mode (-w <dir>)`.

### `crates/server/src/grpc.rs`

`resolve_launch_workspace` (`crates/server/src/grpc.rs:322`) gets one new branch. Today the `!project_key.is_empty() && workspace_dir.is_empty()` arm goes straight to `strategy.acquire_slot`. For a local project that arm must instead return `CoreError::LocalProjectRequiresWorkspace`.

New `CoreError` variant:

```rust
/// A local project was launched without `-w` — there is no pool to fall back on.
LocalProjectRequiresWorkspace { project_key: String },
```

Also: `WorkerStrategy::Code` and `WorkerStrategy::Design` against a local project are rejected regardless of `-w`, because both imply a ticket branch and a workflow. Only `Manual` is permitted. This is the server-side backstop for the CLI check below.

### `crates/ur/src/main.rs`

- `handle_worker_launch` — after `resolve_project_key`, reject the combinations that cannot work:
  - `dispatch && projects[key].is_local()` → error.
  - `mode != "manual"` and the project is local → error.
  - project is local and `workspace.is_none()` → error telling the user to pass `-w .`.
- `process_launch` — the image-resolution `match` (`crates/ur/src/main.rs:845`) needs no change: a local project has `container.image` set, so it takes the `Some(image)` arm.
- `dispatch_ticket` (`crates/ur/src/main.rs:792`) — never reached for local projects after the above.

### `crates/ur/src/project.rs` + `crates/ur/src/output.rs`

`project::add` gains a local path. Signature grows one flag:

```rust
pub fn add(
    config: &ur_config::Config,
    path: &Path,
    image: &str,
    key: Option<&str>,
    name: Option<&str>,
    pool_limit: Option<u32>,
    local: bool,          // new
    output: &OutputManager,
) -> Result<()>;
```

Behavior when `local`: skip `git_remote_origin`, derive the key from the directory basename instead of the repo URL, write `local = true` instead of `repo`, reject `pool_limit`.

New CLI flag on `ProjectCommands::Add` (`crates/ur/src/main.rs:129`):

```
--local    Add a repo-less local project (no git remote, no pool, no dispatch)
```

Output structs become locality-aware:

```rust
pub struct ProjectInfo {
    pub key: String,
    /// `None` for local projects.
    pub repo: Option<String>,
    pub name: String,
    /// `None` for local projects.
    pub pool_limit: Option<u32>,
    pub slots_in_use: usize,
    pub local: bool,
}

pub struct ProjectAdded {
    pub key: String,
    /// `None` for local projects.
    pub repo: Option<String>,
    pub local: bool,
}
```

`project::list` text rendering prints `local` in place of `repo=…` for local projects and omits the pool columns. `project::remove` skips the pool-directory delete for a local project (there is none) — and the existing `--force` requirement can be relaxed there, since nothing destructive happens beyond the `ur.toml` edit.

### `crates/server/src/workflow/handlers/push.rs`

`gh_repo_from_url(&project_config.repo)` at line 697 becomes `require_repo()?`. Unreachable in practice (a local project never gets a workflow), but the compiler forces the decision and the error message documents the invariant.

### `crates/urui/`

No proto change. `Ticket.project` (field 10, `proto/ticket.proto:47`) plus `TuiContext.project_configs` is enough to resolve locality client-side.

- `crates/urui/src/context.rs` — add a helper on `TuiContext`:

  ```rust
  impl TuiContext {
      /// True when the ticket's project is a configured local project.
      pub fn ticket_is_local(&self, ticket: &Ticket) -> bool;
  }
  ```

- `crates/urui/src/components/ticket_table.rs` — `dispatch_label` currently takes only `&Ticket`. It needs locality, so it takes the context (or a precomputed `bool`). A local-project ticket renders with the blocked square (`SYM_BLOCKED`), never the dispatchable one. **Note:** this file has uncommitted changes on the working tree reworking the Status column; rebase this edit onto whatever lands there rather than reverting it.
- `crates/urui/src/update.rs` — the dispatch action for a local-project ticket produces a banner error `Cmd` instead of `Cmd::Dispatch`. Same for the create-and-dispatch action in `create_action_menu`.
- `crates/urui/src/cmd_runner.rs` — `dispatch_ticket` (line 877) and `create_and_dispatch` (line 1127) keep their server-side failure path as the backstop.

## Files to change

```
crates/ur_config/src/lib.rs                       RawProjectConfig.local, repo: Option, validation, is_local/require_repo
crates/ur/src/main.rs                             --local flag; launch-mode guards
crates/ur/src/project.rs                          add(--local), list, remove
crates/ur/src/output.rs                           ProjectInfo, ProjectAdded
crates/server/src/project_registry.rs             is_local()
crates/server/src/pool.rs                         reject local keys before any git work
crates/server/src/grpc.rs                         resolve_launch_workspace branch; CoreError variant
crates/server/src/workflow/handlers/push.rs       require_repo()
crates/urui/src/context.rs                        ticket_is_local()
crates/urui/src/components/ticket_table.rs        dispatch_label locality (rebase onto WIP)
crates/urui/src/update.rs                         dispatch guard + banner
crates/acceptance/tests/e2e.rs                    local-project scenario (line 283 reads proj.repo)
skills/ur:config/SKILL.md                         document local = true
docs/codeflows/config.md                          local projects in the config flow
docs/codeflows/pool-git-builder-flow.md           note local projects never reach the pool
CLAUDE.md (root) + crates/ur_config/CLAUDE.md     locality invariant
```

No new crate dependencies. `toml_edit` (already a `ur` dependency) covers writing `local = true`.

## Test categories to ask for

**`ur_config` unit tests** — parse/validate matrix:
- `local = true` with no `repo` → parses, `is_local()` true, `repo` is `None`
- `local = true` **and** `repo` → error naming both fields
- neither `local` nor `repo` → error mentioning both `repo` and `local = true`
- `local = true` + `pool_limit` → error
- a local project alongside a normal project in one file → both resolve correctly
- local project with full container config (image, mounts, ports), `hostexec`, `hostexec_scripts`, `claude_md`, `brain_dir`, `memory_dir`, `tui.theme` → all resolve identically to a repo-backed project
- `require_repo()` error text names the key

**`server` unit tests:**
- `ProjectRegistry::is_local` for local key / repo key / unknown key
- `RepoPoolManager::acquire_slot` and `acquire_shared_slot` on a local key → `LocalProject` error, and assert **no** builderd RPC was issued (the point of the up-front check)
- `resolve_launch_workspace`: local + `-w` + manual → workspace path, empty slot claim; local without `-w` → `LocalProjectRequiresWorkspace`; local + code mode → rejected; local + design mode → rejected
- `ProjectRegistry::reload` flipping a project between local and repo-backed

**`ur` CLI unit tests:**
- `resolve_project_key` derives a local project from cwd basename
- launch guard matrix: `--dispatch` + local, non-manual mode + local, local without `-w`
- `project add --local` writes `local = true` and no `repo`; key derived from directory basename; rejects `--pool-limit`
- `project add --local` on a directory that *is* a git repo → still local, no `repo` written (explicit flag wins)
- `project remove` on a local project touches no pool directory

**`urui` unit tests:**
- `dispatch_label` for a local-project ticket → blocked square, in every combination of `status`, `dispatch_status`, `children_total`
- dispatch action on a local-project ticket → banner `Cmd`, no `Cmd::Dispatch`

**Acceptance (`crates/acceptance/tests/e2e.rs`)** — per the root CLAUDE.md rule that every new launch mode/flag gets e2e coverage:
- `ur project add --local` → project appears in `ur project list`
- `ur ticket create -p <local>` succeeds
- `ur worker launch -m manual -w <dir> -p <local>` starts a healthy worker on the configured image, with the project's hostexec scripts allowed and its mounts present
- `ur worker launch -p <local>` (no `-w`) fails with the local-project error and creates no pool directory
- `ur worker launch --dispatch` against a local-project ticket fails
- no `pool/<key>` directory exists at any point in the run

## Sequencing

1. `ur_config`: `local` field, `repo: Option`, validation, `is_local`/`require_repo`. Everything downstream is a compile error after this — that error list is the real work list.
2. Fix the compile errors: `pool.rs`, `grpc.rs`, `push.rs`, `project_registry.rs`, `project.rs`, `output.rs`, `e2e.rs`.
3. CLI guards + `--local` flag.
4. TUI locality rendering and dispatch guard.
5. Acceptance tests.
6. Docs: `ur:config` skill, codeflows, CLAUDE.md files.

## Resolved questions

The three open questions were settled during implementation:

- **Workflow-only fields on a local project** — **accepted and ignored.**
  `protected_branches`, `max_implement_cycles`, `max_fix_attempts`,
  `push_again_exit_code`, and `ignored_workflow_checks` all parse and resolve
  normally for a local project; they are simply never consulted. This keeps a
  project flippable between local and repo-backed by editing one line.
  `pool_limit` remains a hard error, because it directly implies a pool.
- **`context_repos`** — **out of scope, but rejected cleanly.** `--context-repos`
  mounts a project's *shared pool clone*, which a local project does not have.
  `acquire_context_repos` now rejects local keys in its up-front validation loop
  with an `InvalidContextRepo` error explaining why, rather than letting the
  shared-slot acquisition fail with a pool error. Mounting a local directory
  read-only as a context repo would be a separate feature.
- **Ticket `--branch`** — **left as a free-text field.** It is harmless metadata,
  and rejecting it would add a locality check to the ticket-update path for no
  practical gain.

## Deviations from the plan

Three things came out differently than sketched, all driven by clippy or by the
existing architecture:

- **`project::add` takes an `AddRequest` struct**, not eight positional arguments —
  clippy's `too_many_arguments` fires at 8. The struct also documents each flag.
- **The TUI guard is split in two.** The plan had a single
  `reject_local_project_dispatch(model, key) -> Result<..., Model>`, which trips
  clippy's `result_large_err` (`Model` is ~2KB). It became a predicate
  (`Model::dispatch_is_blocked_by_locality`) plus a banner producer
  (`update::local_project_dispatch_banner`), which reads better at the call sites
  anyway.
- **`Model.local_projects` carries locality into `update`.** The plan assumed
  `TuiContext` would suffice, but `update` is a pure function with no access to
  it. Render-time locality reads use `TuiContext::project_is_local`; update-time
  reads use the model. Both are documented in `crates/urui/CLAUDE.md`.
- **The server-side decision is a free function** (`local_launch_rejection_reason`)
  rather than only a `LaunchManager` method, so it is unit-testable without
  constructing the whole manager. `grpc.rs` gained its first test module.

## Verification

- `cargo make clippy` — clean (`--workspace --all-targets --all-features -D warnings`).
- `cargo make fmt-fix` — applied.
- `cargo test --workspace` — all green. New tests: 10 in `ur_config`, 6 in
  `ur` (`project.rs`) plus 6 launch-guard tests in `main.rs`, 4 in
  `server/pool.rs`, 2 in `server/project_registry.rs`, 5 in `server/grpc.rs`,
  8 in `urui`.
  - One pre-existing failure surfaced and was **not** a regression:
    `db_events::tests::poller_delivers_event_on_pg_notify` needs the CI postgres on
    `localhost:5433`. `cargo make test:init` starts it; green afterward.
- Acceptance: `scenario_local_project` (config-declared project) and
  `scenario_project_add_local` (`--local` flag + hot reload + remove) added to
  `e2e_all`. Not run here — they need Docker and the built image; the pre-push
  hook runs them.
