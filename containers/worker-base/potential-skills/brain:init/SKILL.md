---
name: brain:init
description: Use when a project brain is empty or untracked under git, or when DREAM.md is missing, lacks a config block, fails ur dream config, blocks brain:dream, or project sources of truth changed
---

# Brain Init & Dream Configuration

Scaffold missing structure in the project's **brain** and create or update `<brain>/DREAM.md` (schema: `dream-md-schema.md` in the `brain:dream` skill's directory).

**Strict scope:** scaffolding is non-destructive and additive only (never overwrite existing brain files). All writes go strictly under `<brain>` — never write project-specific details into any skill or repository file.

## Brain Root & Tools

- `<brain>` is `$UR_BRAIN` (mounted at `/brain/` in workers with a brain).
- Run `ur dream` from your normal workspace directory, **not** from `<brain>` (in a worker, `ur dream` is pinned to the project).
- Run every `git` command from inside `<brain>` (`cd <brain>` first) — never `git -C` (only cwd is host-mapped).

## Part 1: Scaffold Missing Brain Structure

Check each item first; if present, leave untouched and record `"already present"`.

1. **Existing brain check:** if `<brain>/CLAUDE.md` already exists, skip creating `reference/CLAUDE.md` and `working/CLAUDE.md` stubs (respect existing index conventions).
2. **Directories:** ensure `<brain>/`, `<brain>/reference/`, and `<brain>/working/` exist.
3. **`<brain>/CLAUDE.md`** (if missing): short orientation explaining this is the project's user-managed brain, with one-line pointers to `reference/CLAUDE.md` (durable knowledge) and `working/CLAUDE.md` (transient working streams).
4. **`<brain>/reference/CLAUDE.md` and `<brain>/working/CLAUDE.md`** (if missing and step 1 did not skip): empty one-line-pointer index stubs (`- [Title](file.md) — one-line hook`).
5. **Git repository:**
   - If `<brain>/.git` exists, leave it untouched.
   - Otherwise, `cd <brain>`, run `git init`, create `.gitignore` (if missing) ignoring OS/editor clutter (`.DS_Store`, `*.swp`) — **never** ignore `.dream/` — and commit all present files: `git add -A && git commit -m "brain: init"`.
   - Use the user's normal git config (never pass `--no-gpg-sign` or override the author; stop and show error on failure). Never add a remote; push if one already exists.
6. Report created vs already-present paths and any commit made.

## Part 2: Configure or Update `<brain>/DREAM.md`

Run automatically if `<brain>/DREAM.md` is missing or lacks a ```` ```toml ```` config block, or whenever invoked to update dream config.

1. **Read existing files:** `<brain>/DREAM.md` (if already configured, show exact proposed diffs before changing; if legacy table-only or stub, carry values over and confirm), `<brain>/CLAUDE.md`, and its indexes.
2. **Inspect before asking:**
   - Top-level folders → draft `[[folders]]` (`role`, `promote_to` for scratch).
   - Scan brain text for ref-like strings (issue keys, PR refs/URLs, branch names, ticket IDs); group and count candidates.
   - Check which CLI tools actually exist (`command -v`, or `test -x` for absolute paths). Propose only existing read-only check commands; keep uncheckable kinds without `check` so they record as `unknown`.
   - Repos from workspace remotes + cited refs (mark missing local clones `—`; omit commit-message discovery for uncloned repos).
   - Generated/vendored files → draft `off_limits`.
   - Harvest sources: matching `~/.claude/projects/` dirs (local clones and ur pool slots `-…--ur-workspace-pool-<project>-*`), `<ur config>/projects/<project>/transcripts` (offer to create for container workers), and project memory dir; default `since` = 30 days ago.
3. **Confirm one topic at a time** (leading with your draft):
   - Ref kinds: patterns, canonical `id`, read-only argv `check`, state regex, `done` / `not_done` sets.
   - Discovery allowed, sources, and call budget.
   - `[harvest]` directories and `since` (note that `/brain:dream` writes harvested learnings into the brain).
   - `quiet_days` and durable doc review cadence (default 1/run, 30d initial, 7–180d bounds).
   - `off_limits` paths.
4. **Write `<brain>/DREAM.md`:** `## Config` TOML block (per `dream-md-schema.md`) plus prose `Repos` and `Discovery sources` sections. Omit unconfirmed keys rather than guessing.
5. **Validate (mandatory):**
   - `ur dream config` must succeed.
   - For **every** ref kind with `check`, run BOTH `ur dream try <kind> <real-ref>` (must come out done or not done) AND `ur dream try <kind> <made-up-ref>` (must come out `unknown` / `done: null`, NOT not done) and show both outputs before accepting.
6. **Frontmatter rule:** add the scratch-stream frontmatter rule (`status`, `refs` using `DREAM.md` kinds, `last-verified`) to `<brain>/CLAUDE.md` if absent.
7. **Commit:** `cd <brain>`, stage ONLY modified files (`git add DREAM.md CLAUDE.md`), commit `"brain:init: configure dream"`, and `git push` if a remote exists (never add one).
8. **Report:** summarize what was written, mention noticed brain issues, and suggest running `/brain:dream` (do not fix brain content or backfill frontmatter here).

## Guardrails

- Never overwrite existing brain files or create index stubs when `<brain>/CLAUDE.md` already exists.
- Never accept a ref kind without showing `ur dream try` outputs for BOTH a real and a made-up ID.
- Never configure a `check` command that writes, comments, or mutates state.
- Never write helper scripts into `<brain>` (mechanical logic belongs in `ur dream`) or leak project specifics outside `<brain>`.
- Never ignore `.dream/` in `.gitignore`, never run `git -C`, never add a remote, never force-push or pass `--no-gpg-sign`.
<!-- compiled-from: skills/specs/brain:init.md@9e06cea5969e profile:worker@6afed0c52191 -->
