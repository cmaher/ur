# brain

- **Targets:** worker, local
- **Trigger:** Loading durable project knowledge (architecture, decisions, domain facts) or transient in-progress working context stored outside the repo in the project's brain.

## Core
- **Purpose:** read-oriented access to the per-project, user-managed brain outside the repo (`reference/` for durable knowledge, `working/` for transient streams). Content is user-owned — never restructure or delete brain content here (`/brain:init` scaffolds, `/brain:dream` consolidates).
- **Reading workflow:**
  1. Read `<brain>/CLAUDE.md` (if missing, report that the brain is empty, suggest `/brain:init`, and stop).
  2. Follow table-of-contents pointers (`reference/CLAUDE.md`, `working/CLAUDE.md`, one-line pointer indexes).
  3. Load **only** the entries relevant to the current task — never dump the whole brain indiscriminately.
- **Writing rules (when adding notes/streams):**
  - New working stream (`working/<stream>/README.md` or single file) gets YAML frontmatter: `status: active`, `refs:` (using ref kinds from `DREAM.md` when present), `last-verified: YYYY-MM-DD`.
  - **Code-that-runs check:** before recording a code fact, verify call sites and the flag/config enabling it per environment. Unused code or off-by-default features must be labeled as such.
  - Cite code by **path and symbol**, never `file:line` (line numbers rot).
  - **Git hygiene:** `cd <brain>`, stage ONLY the files you changed, commit with `"brain: <what changed>"`. If `git remote` has a remote, `git push`; otherwise stop after commit. Never add a remote or force-push.

## Worker
- Brain root is `$UR_BRAIN` (`/brain`).
- Always `cd <brain>` before git commands (never `git -C` — only cwd is host-mapped).

## Local
- Brain root is `$UR_BRAIN` if set; else resolve the project's `brain` path from `ur project list --output json` (explicit arg or matching this repo's `origin` remote; ask if none or multiple match).

## Guardrails
- Never read the whole brain when only specific pointers apply.
- Never restructure or delete existing brain files from this skill.
- Never cite `file:line` or record unverified/dead code as live behavior.
- Never stage untouched files, add remotes, or force-push.
