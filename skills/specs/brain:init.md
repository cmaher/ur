# brain:init

- **Targets:** worker, local
- **Trigger:** Initializing an empty/untracked project brain, or creating/updating `<brain>/DREAM.md` when it is missing, has no config block, `ur dream config` fails, `brain:dream` cannot run, or project sources of truth changed.
- **Merges:** `brain:init` + `brain:dream-init`
- **Assets:** references `dream-md-schema.md` (stored alongside `brain:dream`)

## Core
- **Strict scope:** non-destructive and additive for scaffolding; writes **only** under `<brain>` (never project-specific info into any skill or repo file).
- **Part 1 — Scaffold missing brain structure (only create what is missing; never overwrite):**
  - If `<brain>/CLAUDE.md` already exists, skip creating `reference/CLAUDE.md` and `working/CLAUDE.md` stubs (respect existing index conventions).
  - Ensure `<brain>/`, `<brain>/reference/`, `<brain>/working/` exist.
  - Create `<brain>/CLAUDE.md` (orientation + links to `reference/CLAUDE.md` and `working/CLAUDE.md`), `reference/CLAUDE.md`, and `working/CLAUDE.md` (one-line pointer index stubs) only where missing.
  - Git repo: if `<brain>/.git` is absent, `cd <brain>`, `git init`, create `.gitignore` (OS/editor clutter like `.DS_Store`, `*.swp`; **never** ignore `.dream/`), commit `"brain: init"` using normal git config (no `--no-gpg-sign` or author override; stop on error). Never add a remote; push if one already exists.
  - Report created vs already-present paths and any commit made.
- **Part 2 — Configure or update `<brain>/DREAM.md` (run automatically if `DREAM.md` has no `toml` config block, or whenever invoked to update dream config):**
  - Read `<brain>/DREAM.md` (if already configured, show diffs before changing; if legacy table-only or stub, migrate values and confirm), `<brain>/CLAUDE.md`, and its indexes.
  - **Inspect before asking:**
    - Top-level folders → draft `[[folders]]` (`role`, `promote_to` for scratch).
    - Scan brain text for ref-like patterns (issue keys, PR refs/URLs, branches, ticket IDs), group & count candidates.
    - Check which CLI tools actually exist (`command -v` / `test -x`); only propose existing read-only check commands (keep uncheckable kinds without `check` so they record as `unknown`).
    - Repos from workspace remotes + cited refs (mark missing local clones `—`).
    - Generated/vendored files → draft `off_limits`.
    - Harvest sources: matching `~/.claude/projects/` dirs (local clones + ur pool slots), `<ur config>/projects/<project>/transcripts` (offer to create for container workers), and project memory dir; default `since` = 30 days ago.
  - **Confirm with user one topic at a time** (with draft defaults): ref kinds (patterns, canonical `id`, read-only argv `check`, state extraction, done/not_done sets), discovery sources & call budget, `[harvest]` dirs & `since` (noting dream writes harvested learnings into brain), `quiet_days` & review cadence (default 1/run, 30d initial, 7–180d bounds), `off_limits`.
  - Write `<brain>/DREAM.md` (`## Config` TOML per `dream-md-schema.md` + prose `Repos` / `Discovery sources`; omit unconfirmed keys rather than guessing).
  - **Mandatory validation:**
    1. `ur dream config` must succeed.
    2. For EVERY ref kind with `check`, run `ur dream try <kind> <real-ref>` (must be done or not_done) AND `ur dream try <kind> <made-up-ref>` (must be `unknown` / `done: null`, NOT not_done) and show both outputs before accepting.
  - Add frontmatter rule (`status`, `refs`, `last-verified`) to `<brain>/CLAUDE.md` if absent.
  - Stage only modified files (`git add DREAM.md CLAUDE.md`), commit `"brain:init: configure dream"`, push if remote exists.
  - Report what was written, mention noticed brain issues, and suggest running `brain:dream` (do not fix brain content or backfill frontmatter here).

## Worker
- Brain root is `$UR_BRAIN` (`/brain`); `ur dream` is pinned to the worker's project (run from workspace, not `<brain>`); git commands run via `cd <brain>` (never `git -C`).

## Local
- Brain root is `$UR_BRAIN` if set, else `ur dream path -p <key>` / `ur project list --output json` matching explicit arg or `origin` remote (ask if ambiguous). Pass `-p <key>` to `ur dream` commands from the workspace directory; run git via `cd <brain>`.

## Guardrails
- Never overwrite existing brain files or create index stubs when `<brain>/CLAUDE.md` already exists.
- Never accept a ref kind without showing `ur dream try` output for BOTH a real and a made-up ID.
- Never configure a `check` command that mutates state, comments, or writes.
- Never write helper scripts into `<brain>` (mechanical logic belongs in `ur dream`) or leak project specifics into skill/repo files.
- Never ignore `.dream/` in `.gitignore`, never add a remote, never force-push or bypass git signing.
