# Skill Loading

How skills get baked into the container image and selectively activated at runtime.

## Skill Sources

Two directories in the **base** container build context supply skills:

- `containers/worker-base/vendor/superpowers/skills/` — upstream/third-party skills
- `containers/worker-base/potential-skills/` — project-specific skills and overrides

Both are merged into a single `potential-skills/` pool during the Docker build. **`potential-skills/` copies second, so project-specific versions override vendor skills with the same name.**

These sources are agent-agnostic and live in the base image (`ur-worker-base:latest`), not an
agent-specific Claude, Codex, or AGY layer. Every agent image built on the base inherits them.

There is one `SKILL.md` dialect and one shared `potential-skills/` source. The destination is
`~/{agent.customization_root()}/{agent.skill_subdir()}/`: `~/.claude/skills/`,
`~/.codex/skills/`, or `~/.gemini/config/skills/`. AGY is the reason
`customization_root()` is distinct from `home_subdir()`; its runtime settings and state remain
under `~/.gemini/antigravity-cli/` while ur-managed customizations live in
`~/.gemini/config/`.

## Build Time (Dockerfile)

```
COPY vendor/superpowers/skills/ /home/worker/.agent-shared/potential-skills/
COPY potential-skills/          /home/worker/.agent-shared/potential-skills/
```

All skills land in `~/.agent-shared/potential-skills/`. The agent's own skill directory (e.g. `~/.claude/skills/` for Claude) starts empty — skills are not active until explicitly selected at runtime.

## Mode Resolution (ur-server)

When a process launches, the server resolves which skills, model, and agent to use.

```
WorkerLaunchRequest { mode, skills, agent_type }
    │
    ▼
WorkerManager::resolve_mode(mode, requested_agent)     [crates/server/src/worker.rs]
    │   Returns ResolvedMode { strategy, skills, model, agent }:
    │     1. Mode name → WorkerModesConfig lookup (default: "code")
    │     2. Strategy from built-in or custom mode's `base` field
    │     3. Skills: explicit `skills` param > mode's skill list > code defaults
    │     4. Model: mode's `model` field, else the three-level chain below
    │     5. Agent: explicit `requested_agent` param > mode's `agent` field > top-level agent > claude
    │
    ▼
UR_WORKER_SKILLS env var set on container       (comma-separated skill names)
UR_WORKER_MODEL env var set on container        (model name, e.g. "sonnet", "opus")
UR_AGENT_TYPE env var set on container          ("claude", "codex", or "agy")
```

### Default Modes (hardcoded, overridable via ur.toml)

Default skill lists are defined in `crates/server/src/strategy.rs` (`WorkerStrategy::skills()`, `common_skills()`). Default models are defined per agent in `crates/ur_config/src/agent.rs` (`AgentType::default_model()`). See those files for the current lists.

| Mode | Claude | Codex | AGY |
|------|--------|-------|-----|
| code | sonnet | gpt-5.6-terra | gemini-3.8-flash |
| design | opus | gpt-5.6-sol | gemini-3.8-flash |
| manual | opus | gpt-5.6-sol | gemini-3.8-flash |

### ur.toml Override

```toml
[worker_modes.code]
skills = ["tickets", "custom-skill"]

[worker_modes.my-mode]
base = "design"
skills = ["a", "b", "c"]
model = "my-custom-model"    # overrides the base strategy's default
agent = "claude"             # optional; defaults to "claude" when omitted
```

Config-defined modes merge with defaults: defined names replace their default counterpart, undefined defaults are preserved. Each mode may optionally specify a `model` field to override the base strategy's default model, and an `agent` field to override which agent runs it (an unrecognized `agent` value is a config error naming the offending mode).

### `[worker_models.<agent>]` Override (strategy-level default)

```toml
[worker_models.claude]
code   = { model = "opus" }
design = { model = "sonnet", effort = "high" }
manual = { model = "sonnet" }

[worker_models.codex]
code = { effort = "high" }
```

`[worker_models]` (parsed in `WorkerModesConfig::from_toml`, `crates/server/src/worker.rs`) overrides the built-in default model and reasoning effort per **strategy** ("code", "design", "manual") rather than per mode. It changes what every mode based on that strategy resolves to when it doesn't specify its own `model`/`effort` — including the built-in `code`/`design` modes themselves.

Every top-level key must be an **agent table** (`claude`, `codex`, or `agy`); a bare `code = "opus"` at the top level, or under an agent, is a hard config error telling you to use `[worker_models.<agent>]` with inline `{ model, effort }` entries. `deny_unknown_fields` rejects typos or unrecognized strategy names with an error naming the bad key, an unknown agent table is rejected with the valid agent list, and an `effort` value outside `agent.supported_efforts()` is rejected at parse time. Every key is optional; `model` and `effort` resolve independently, so an entry may set just one.

### Model / Effort Resolution Precedence (highest wins)

Each field resolves on its own:

1. A custom mode's explicit `worker_modes.<name>.model` / `.effort`
2. The `[worker_models.<agent>].<strategy>` entry's `model` / `effort` for that mode's base strategy
3. `agent.default_model(strategy)` — the agent's own built-in table — and `agent.default_effort()` (`"medium"` for all agents)

Resolution happens after the agent is resolved, so an explicit `--agent` launch override picks up that agent's tables and defaults. The resolved values reach the container as `UR_WORKER_MODEL` and `UR_WORKER_EFFORT` and are turned into agent-specific flags by `agent.spawn_command(model, effort)` (`--model`/`--effort` for Claude, `-m`/`-c model_reasoning_effort` for Codex, and `--model`/`--effort` plus `--dangerously-skip-permissions` for AGY). They are not injected into settings files.

## Container Startup (entrypoint.sh → workerd init)

```
entrypoint.sh
    │
    ▼
workerd init                          [crates/workerd/src/init/{skills,instructions,settings}.rs]
    │   let agent = AgentType::from_env();   // reads UR_AGENT_TYPE, defaults to claude
    │
    ├─ InitSkillsManager::run()            [init/skills.rs]
    │     1. Wipe ~/{agent.customization_root()}/{agent.skill_subdir()}/ (remove + recreate)
    │     2. Read UR_WORKER_SKILLS env var
    │     3. For each comma-separated skill name:
    │        - src: ~/.agent-shared/potential-skills/<name>/
    │        - dst: ~/{agent.customization_root()}/{agent.skill_subdir()}/<name>/
    │        - Recursive directory copy (preserves subdirs)
    │        - Missing skills log a warning, don't fail
    │
    ├─ InitInstructionsManager::run()      [init/instructions.rs]
    │     (compose strategy instruction file + shared fragments — see below)
    │
    ├─ InitSettingsManager::run()          [init/settings.rs]
    │     Gated on agent.settings_filename().is_some().
    │     1. Read ~/{agent.home_subdir()}/potential-settings.json (baked in at build time)
    │     2. Copy it VERBATIM to ~/{agent.home_subdir()}/{agent.settings_filename()}
    │        — no model merge. The model reaches the agent only via the
    │        `--model` launch flag (agent.spawn_command()), never settings.json.
    │
    ▼
~/.claude/skills/ now contains only the requested skills (Claude values shown)
~/.claude/settings.json is a verbatim copy of the baked-in file
    │
    ▼
Claude Code reads ~/.claude/skills/ and ~/.claude/settings.json at session start
```

`crates/workerd/src/init/mod.rs` holds the shared `copy_dir_recursive`/`collect_md_files` helpers used by the skills and instructions managers. Source paths under `.agent-shared/` are plain literals (agent-agnostic, baked by the base image); destinations are agent-derived via `AgentType`.

## Per-Strategy Instruction File Delivery

Some capabilities (e.g., ticket management) are delivered as instruction-file content (e.g. `CLAUDE.md`) rather than skills. This avoids the skill loading overhead and puts instructions directly in the worker's system context.

### Build Time

```
COPY instructions/        /home/worker/.agent-shared/instructions/
COPY shared-instructions/ /home/worker/.agent-shared/shared-instructions/
```

- `instructions/` contains one `{strategy}.md` file per strategy (e.g., `code.md`, `design.md`, `manual.md`) — the layout is flat; agent-specific naming comes from `agent.instruction_filename()` at write time, not from the source filenames
- `shared-instructions/` contains `.md` fragments included in all strategies (e.g., `tickets.md`)

### Runtime (workerd init)

```
InitInstructionsManager::run()
    1. Read UR_WORKER_INSTRUCTION_STRATEGY env var (set by server from WorkerStrategy)
    2. Read ~/.agent-shared/instructions/{value}.md as strategy content
    3. Append all ~/.agent-shared/shared-instructions/*.md files (sorted alphabetically)
    4. If UR_PROJECT_INSTRUCTION names a readable file, resolve %WORKSPACE% in it,
       write it to ~/{agent.home_subdir()}/PROJECT_{agent.instruction_filename()},
       and append an `@<path>` reference to the composed output
    5. Write composed result to ~/{agent.home_subdir()}/{agent.instruction_filename()}
       (e.g. ~/.claude/CLAUDE.md for Claude)
    6. Missing strategy file → warning (non-fatal); empty/unset env var → skip entirely
```

## Host Skills (Runtime Injection via ur.toml)

In addition to skills baked into the container image at build time, operators can inject skills from the host machine at runtime via the `[skills]` section of `ur.toml`. These host skills are bind-mounted read-only into the container's `.agent-shared/potential-skills/` directory and become available for selection alongside baked-in skills.

### Schema

```toml
[skills.common]
my-skill = "%URCONFIG%/skills/my-skill"
internal-tool = "/opt/skills/internal-tool"

[skills.code]
research-helper = "%URCONFIG%/skills/research-helper"

[skills.design]
research-helper = "%URCONFIG%/skills/research-helper"
```

Three sub-tables are supported:

| Sub-table | Purpose |
|-----------|---------|
| `[skills.common]` | Skills available to all modes |
| `[skills.code]` | Skills available only to `code`-strategy modes |
| `[skills.design]` | Skills available only to `design`-strategy modes |

Each entry maps a skill name to a host path. Paths support two forms:

- `%URCONFIG%/...` — resolves to `<config_dir>/...` (recommended; portable across machines)
- `/absolute/path` — literal host-side path

### Merge Order

`WorkerManager::merge_global_skills()` (`crates/server/src/worker.rs`) merges the three sub-tables before launch:

```
mode_skills (from [skills.code] or [skills.design])
    +
common_skills (from [skills.common])
    =
merged host skills map (name → host path)
```

Keys in mode-specific tables shadow same-named keys in `common`. The merged map is then passed to `RunOptsBuilder::add_extra_skills()`.

### Bind-Mount Target

Each host skill directory is mounted into the container at:

```
/home/worker/.agent-shared/potential-skills/<name>  (read-only)
```

`.agent-shared` is agent-agnostic baked content, so this mount target needs no `AgentType` parameterization — it is the same literal for every agent. This is the same directory tree where baked-in skills live, so `workerd init` treats host skills and image skills identically when copying to `~/{agent.home_subdir()}/{agent.skill_subdir()}/` at startup.

### Override-of-Baked Semantics

Because host skills are mounted directly into `.agent-shared/potential-skills/`, a host skill with the same name as a baked-in skill **shadows** the baked version. The bind-mount is applied after the image layer, so the host path wins. This allows operators to patch or replace a shipped skill without rebuilding the image.

### Server-Container Visibility Caveat

Host skill paths must be visible to the **server process** at mount-time, not just the worker container. When the server itself runs inside a container (as in the standard Docker Compose topology), a path like `/Users/me/skills/my-skill` on the developer's Mac is not accessible from inside the server container.

Prefer `%URCONFIG%/...` paths stored under `~/.ur/` (or wherever `$UR_CONFIG` points) because that directory is already volume-mounted into the server container. Absolute paths outside the server's mount namespace will silently produce empty mounts.

## Key Files

| File | Role |
|------|------|
| `containers/worker-base/Dockerfile` | Bakes skill sources and instruction files into `.agent-shared/` in the base image |
| `containers/worker-base/potential-skills/` | Project-specific skills (override vendor) |
| `containers/worker-base/instructions/` | Per-strategy instruction files (`code.md`, `design.md`, `manual.md`) |
| `containers/worker-base/shared-instructions/` | Instruction-file fragments shared across all strategies |
| `containers/worker-base/vendor/superpowers/skills/` | Upstream/third-party skills |
| `containers/worker-claude/entrypoint.sh` | Calls `workerd init` at container start |
| `crates/workerd/src/init/skills.rs` | Copies skills into the agent's own skill directory |
| `crates/workerd/src/init/instructions.rs` | Composes the strategy instruction file + shared fragments + project reference |
| `crates/workerd/src/init/settings.rs` | Verbatim-copies the baked settings file (no model merge) |
| `crates/ur_config/src/agent.rs` | `AgentType`/`AgentAuth` — per-agent home subdir, skill subdir, instruction filename, default models |
| `crates/server/src/strategy.rs` | Default skill lists per mode (`WorkerStrategy::skills()`, `common_skills()`) |
| `crates/server/src/worker.rs` | Mode resolution, `[worker_models]` parsing (`RawWorkerModels`, `effective_default_model()`), injects UR_WORKER_SKILLS, UR_WORKER_INSTRUCTION_STRATEGY, UR_WORKER_MODEL, and UR_AGENT_TYPE env vars |

## Skill Hook Integration

Some skills support project-specific customization via skill hooks. These hooks are Markdown files that skills reference via `@` directives — Claude Code expands them inline at session start. The hook files are delivered via the two-layer overlay described in [project-file-mounting.md](project-file-mounting.md#skill-hooks).

### Skills with Hook Support

| Skill | Hook Directory | Hook Files | Purpose |
|-------|---------------|------------|---------|
| `implement` | `implement/` | `after-ticket-claim.md`, `subtask-verifications.md`, `before-dispatch.md`, `final-verifications.md` | Verification commands at workflow checkpoints |
| `code-review` | `code-review/` | `review-guidelines.md` | Project-specific review rules (rendered under "Project Rules" header) |

To add project-specific hooks, place files in `ur-hooks/skills/<skill>/` in the project repo. Host overlays at `<config_dir>/projects/<key>/hooks/skills/<skill>/` take precedence on conflict.
