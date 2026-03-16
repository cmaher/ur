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

When a process launches, the server resolves which skills to activate.

```
WorkerLaunchRequest { mode, skills }
    │
    ▼
WorkerManager::resolve_skills()                [crates/server/src/worker.rs]
    │   Priority:
    │     1. If `skills` is non-empty → use directly
    │     2. If `mode` is non-empty → look up prompt_modes.<mode>.skills
    │     3. Otherwise → use prompt_modes.code (default)
    │
    ▼
UR_WORKER_SKILLS env var set on container       (comma-separated skill names)
```

### Default Modes (hardcoded, overridable via ur.toml)

Default skill lists for each mode are defined in `crates/server/src/strategy.rs` (`WorkerStrategy::skills()` and `common_skills()`). See that file for the current lists.

### ur.toml Override

```toml
[prompt_modes.code]
skills = ["tickets", "custom-skill"]

[prompt_modes.my-mode]
skills = ["a", "b", "c"]
```

Config-defined modes merge with defaults: defined names replace their default counterpart, undefined defaults are preserved.

## Container Startup (entrypoint.sh → workerd init)

```
entrypoint.sh
    │
    ▼
workerd init                                     [crates/workerd/src/init_skills.rs]
    │   InitSkillsManager::init_skills()
    │     1. Wipe ~/.claude/skills/ (remove + recreate)
    │     2. Read UR_WORKER_SKILLS env var
    │     3. For each comma-separated skill name:
    │        - src: ~/.claude/potential-skills/<name>/
    │        - dst: ~/.claude/skills/<name>/
    │        - Recursive directory copy (preserves subdirs)
    │        - Missing skills log a warning, don't fail
    │
    ▼
~/.claude/skills/ now contains only the requested skills
    │
    ▼
Claude Code reads ~/.claude/skills/ at session start
```

## Key Files

| File | Role |
|------|------|
| `containers/claude-worker/Dockerfile` | Bakes both skill sources into potential-skills/ |
| `containers/claude-worker/all-skills/` | Project-specific skills (override vendor) |
| `containers/claude-worker/vendor/superpowers/skills/` | Upstream/third-party skills |
| `containers/claude-worker/entrypoint.sh` | Calls `workerd init` at container start |
| `crates/workerd/src/init_skills.rs` | Copies selected skills from potential-skills/ to skills/ |
| `crates/server/src/strategy.rs` | Default skill lists per mode (`WorkerStrategy::skills()`, `common_skills()`) |
| `crates/server/src/worker.rs` | Mode resolution, injects UR_WORKER_SKILLS env var |
