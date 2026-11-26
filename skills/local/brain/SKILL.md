---
name: brain
description: Use when loading durable project knowledge (architecture, decisions, domain facts) or transient in-progress working context stored outside the repo in the project's brain
---

# Brain

The **brain** is a per-project, user-managed context directory outside the repo (`reference/` for durable knowledge, `working/` for transient streams). Its content is user-owned — this skill is read-oriented and never restructures or deletes brain content (`/brain:init` scaffolds missing layout; `/brain:dream` consolidates).

## Brain Root

`<brain>` is `$UR_BRAIN` when set. Otherwise resolve the project's `brain` field from `ur project list --output json` — using the project key passed in `$ARGUMENTS`, or matching this repo's `origin` remote (if none or multiple match, ask the user). Run brain git commands from inside `<brain>` (`cd <brain>` first).

## Reading the Brain

1. **Read `<brain>/CLAUDE.md`.** If it does not exist, report that the brain is empty, suggest running `/brain:init`, and stop.
2. **Follow table-of-contents pointers** in `<brain>/CLAUDE.md` — typically:
   - `<brain>/reference/CLAUDE.md` — durable knowledge (architecture, decisions, domain facts).
   - `<brain>/working/CLAUDE.md` — transient, in-progress context (investigations, scratch streams).
3. **Load only what applies to the current task.** Follow specific one-line index pointers rather than reading the whole brain indiscriminately.

## Writing Notes or Streams

- **New working stream** (`<brain>/working/<stream>/README.md` or a single file) must carry YAML frontmatter:
  ```yaml
  ---
  status: active          # active | done | archived
  refs:                   # tickets, PRs, branches; use ref kinds from DREAM.md when present
    - ticket: EXAMPLE-123
  last-verified: YYYY-MM-DD
  ---
  ```
- **Verify code actually runs:** before recording a code fact, check its call sites and the flag/config that enables it per environment. Unused code or off-by-default features must be labeled as such, never as live behavior.
- **Cite by path and symbol, never `file:line`** (line numbers go stale).
- **Git hygiene:** `cd <brain>`, stage **only** the files you changed (`git add <files>`), and commit with `"brain: <what changed>"`. If `git remote` lists a remote, run `git push`; otherwise stop after committing. Never add a remote or force-push.

## Guardrails

- Never dump the whole brain when only specific entries are relevant.
- Never restructure or delete existing brain files here.
- Never cite `file:line` or record unverified/dead code as live behavior.
- Never stage untouched files, add remotes, or force-push.

$ARGUMENTS
<!-- compiled-from: skills/specs/brain.md@3f7673200aeb profile:local@daa4419e0589 -->
