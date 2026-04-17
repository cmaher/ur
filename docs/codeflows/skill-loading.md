# Skill Loading

How skills get baked into the container image and selectively activated at runtime.

## Skill Sources

Two directories in the container build context supply skills:

- `containers/claude-worker/vendor/superpowers/skills/` — upstream/third-party skills
- `containers/claude-worker/all-skills/` — project-specific skills and overrides

Both are merged into a single `potential-skills/` pool during the Docker build. **all-skills/ copies second, so project-specific versions override vendor skills with the same name.**

## Build Time (Dockerfile)

```
COPY vendor/superpowers/skills/ /home/worker/.claude/potential-skills/
COPY all-skills/               /home/worker/.claude/potential-skills/
RUN mkdir -p /home/worker/.claude/skills
```

All skills land in `~/.claude/potential-skills/`. The `~/.claude/skills/` directory starts empty — skills are not active until explicitly selected at runtime.

## Mode Resolution (ur-server)

When a process launches, the server resolves which skills and model to use.

```
WorkerLaunchRequest { mode, skills }
    │
    ▼
WorkerManager::resolve_mode()                  [crates/server/src/worker.rs]
    │   Returns (WorkerStrategy, skills, model):
    │     1. Mode name → WorkerModesConfig lookup (default: "code")
    │     2. Strategy from built-in or custom mode's `base` field
    │     3. Skills: explicit `skills` param > mode's skill list > code defaults
    │     4. Model: mode's `model` field (or base strategy's default_model())
    │
    ▼
UR_WORKER_SKILLS env var set on container       (comma-separated skill names)
UR_WORKER_MODEL env var set on container        (Claude Code model alias, e.g. "sonnet", "opus")
```

### Default Modes (hardcoded, overridable via ur.toml)

Default skill lists and models for each mode are defined in `crates/server/src/strategy.rs` (`WorkerStrategy::skills()`, `common_skills()`, and `default_model()`). See that file for the current lists.

| Mode | Default Model |
|------|---------------|
| code | sonnet |
| design | opus |

### ur.toml Override

```toml
[worker_modes.code]
skills = ["tickets", "custom-skill"]

[worker_modes.my-mode]
base = "design"
skills = ["a", "b", "c"]
model = "my-custom-model"    # overrides the base strategy's default
```

Config-defined modes merge with defaults: defined names replace their default counterpart, undefined defaults are preserved. Each mode may optionally specify a `model` field to override the base strategy's default model.

## Container Startup (entrypoint.sh → workerd init)

```
entrypoint.sh
    │
    ▼
workerd init                                     [crates/workerd/src/init_skills.rs]
    │
    ├─ InitSkillsManager::init_skills()
    │     1. Wipe ~/.claude/skills/ (remove + recreate)
    │     2. Read UR_WORKER_SKILLS env var
    │     3. For each comma-separated skill name:
    │        - src: ~/.claude/potential-skills/<name>/
    │        - dst: ~/.claude/skills/<name>/
    │        - Recursive directory copy (preserves subdirs)
    │        - Missing skills log a warning, don't fail
    │
    ├─ InitSkillsManager::init_claude_md()
    │     (compose strategy CLAUDE.md + shared fragments)
    │
    ├─ InitSkillsManager::init_settings_json()
    │     1. Read ~/.claude/potential-settings.json (baked in at build time)
    │     2. Read UR_WORKER_MODEL env var
    │     3. If non-empty: merge "model": "<value>" into JSON object
    │     4. If empty/missing: write base file unchanged (no model key)
    │     5. Write result to ~/.claude/settings.json
    │
    ▼
~/.claude/skills/ now contains only the requested skills
~/.claude/settings.json has the resolved model (if any)
    │
    ▼
Claude Code reads ~/.claude/skills/ and ~/.claude/settings.json at session start
```

## Per-Strategy CLAUDE.md Delivery

Some capabilities (e.g., ticket management) are delivered as CLAUDE.md content rather than skills. This avoids the skill loading overhead and puts instructions directly in the worker's system context.

### Build Time

```
COPY all-claudes/   /home/worker/.claude/potential-claudes/
COPY shared-claudes/ /home/worker/.claude/shared-claudes/
```

- `all-claudes/` contains one `{strategy}.md` file per strategy (e.g., `code.md`, `design.md`)
- `shared-claudes/` contains `.md` fragments included in all strategies (e.g., `tickets.md`)

### Runtime (workerd init)

```
InitSkillsManager::init_claude_md()
    1. Read UR_WORKER_CLAUDE env var (set by server from WorkerStrategy)
    2. Read ~/.claude/potential-claudes/{value}.md as strategy content
    3. Append all ~/.claude/shared-claudes/*.md files (sorted alphabetically)
    4. Write composed result to ~/.claude/CLAUDE.md
    5. Missing strategy file → warning (non-fatal)
```

## Key Files

| File | Role |
|------|------|
| `containers/claude-worker/Dockerfile` | Bakes skill sources, strategy CLAUDEs, and shared CLAUDEs into image |
| `containers/claude-worker/all-skills/` | Project-specific skills (override vendor) |
| `containers/claude-worker/all-claudes/` | Per-strategy CLAUDE.md files |
| `containers/claude-worker/shared-claudes/` | CLAUDE.md fragments shared across all strategies |
| `containers/claude-worker/vendor/superpowers/skills/` | Upstream/third-party skills |
| `containers/claude-worker/entrypoint.sh` | Calls `workerd init` at container start |
| `crates/workerd/src/init_skills.rs` | Copies skills, composes strategy CLAUDE.md, writes settings.json with model |
| `crates/server/src/strategy.rs` | Default skill lists and models per mode (`WorkerStrategy::skills()`, `common_skills()`, `default_model()`) |
| `crates/server/src/worker.rs` | Mode resolution, injects UR_WORKER_SKILLS, UR_WORKER_CLAUDE, and UR_WORKER_MODEL env vars |
