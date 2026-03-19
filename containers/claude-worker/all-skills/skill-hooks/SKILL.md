---
name: skill-hooks
description: "Use when creating or modifying project-specific skill hooks — the verification snippets embedded by the implement skill at key workflow checkpoints"
---

# Skill Hooks

Skill hooks are short Markdown snippets that the `implement` skill embeds at key workflow checkpoints via `@` directives. They tell agents what verification to run (and when) before proceeding.

## How They Work

The implement skill references hooks at these points:

| Hook file | When it runs | Typical use |
|-----------|-------------|-------------|
| `implement/after-ticket-claim.md` | After claiming a single ticket | Project-specific setup or context loading |
| `implement/subtask-verifications.md` | Before each commit (single tickets + subagents) | Fast checks: linter, type checker, compiler diagnostics |
| `implement/before-dispatch.md` | Before dispatching subagents (epics only) | Verify codebase is clean before parallelizing |
| `implement/final-verifications.md` | After all subagents complete (epics only) | Full CI: format, lint, build, test |

At runtime, the implement SKILL.md contains lines like:
```
@/home/worker/.claude/skill-hooks/implement/subtask-verifications.md
```
Claude Code expands these inline, injecting the snippet's content into the skill prompt.

## Where Hooks Live

Hooks are **project-specific** — they live in the project repo and get copied into the container at startup.

**Default convention:** `ur-hooks/skills/` in the project root. If no `skill_hooks_dir` is configured in `ur.toml`, workerd automatically looks for `/workspace/ur-hooks/skills/` and copies it if present.

**Explicit config** in `ur.toml`:
```toml
[projects.myproject]
skill_hooks_dir = "%PROJECT%/ur-hooks/skills"
```

The directory structure mirrors the target path inside `~/.claude/skill-hooks/`:
```
ur-hooks/skills/
  implement/
    after-ticket-claim.md
    subtask-verifications.md
    before-dispatch.md
    final-verifications.md
```

## Writing Good Hooks

### Principles

1. **Be specific about commands.** Name the exact command to run, not "run the linter". Agents follow instructions literally.
2. **Use project task runners.** Prefer `cargo make clippy` over `cargo clippy --workspace --all-targets --all-features -- -D warnings`. The task runner is the source of truth for flags.
3. **Distinguish fast vs full checks.** Subtask hooks run per-commit and should be fast (seconds). Final hooks run once and can be thorough (minutes).
4. **State the gate clearly.** End each hook with an unambiguous "do NOT proceed until X" statement.
5. **Handle fallbacks.** If a fast check depends on a background process (like bacon), specify what to run if the process isn't available.
6. **Empty hooks are fine.** If a checkpoint needs no verification, leave the file empty. The `@` directive silently expands to nothing.

### Fast check pattern (subtask-verifications, before-dispatch)

Good for compiler/linter diagnostics that a background watcher maintains:

```markdown
Before committing, verify the codebase has no compiler errors or warnings.

1. Read `.bacon-locations` in the workspace root.
2. If the file exists and is non-empty, check every line for `error` or `warning` kinds.
   - If ANY errors or warnings are present, fix ALL of them before proceeding.
   - Re-read `.bacon-locations` after fixes to confirm it is clean.
3. If `.bacon-locations` does not exist or is empty, fall back to running:
   `cargo make clippy`
   Fix any errors or warnings reported, then re-run to confirm clean output.

Do NOT commit until all diagnostics are clean.
```

### Full CI pattern (final-verifications)

Good for the end-of-epic gate where thoroughness matters more than speed:

```markdown
After all subagents complete, run full CI verification:

1. Run `cargo make ci-fmt-fix` (silently auto-formats, then runs clippy, build, tests).
   Fix any failures and re-run until clean.
2. `ci-fmt-fix` may have reformatted files. Check for unstaged changes and,
   if any, stage and commit them before proceeding.
```

### Setup pattern (after-ticket-claim)

Good for loading project context or activating tools:

```markdown
After claiming the ticket, run `make dev-setup` to ensure local dependencies are up to date.
```

Or leave empty if no setup is needed.

## Adding Hooks to a New Project

1. Create the directory: `mkdir -p ur-hooks/skills/implement`
2. Create the hook files you need (empty files are fine as placeholders)
3. Optionally add `skill_hooks_dir = "%PROJECT%/ur-hooks/skills"` to the project's `ur.toml` entry (not required if using the default convention)
4. Commit the hooks to the repo

$ARGUMENTS
