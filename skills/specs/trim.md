# trim

- **Targets:** worker, local
- **Trigger:** After a change is functionally done and before it ships, to find and remove unnecessary code — duplicates of existing repo/platform code, pass-through wrappers, unneeded guards/knobs, repeated test setup, dead code, and stale docs.

## Core
- **Single question:** *what can go?* Cut by deleting, replacing with existing codebase/platform code, or folding copies into one. Never hunt bugs or add guards (that's `code-review`).
- **Invocation arguments:**
  - Range (`<base>..<head>`, PR number, or branch; default: working branch vs merge base with default branch).
  - `apply`: make accepted cuts, verify, and commit. Without `apply`, **report only**.
  - Optional scope contracts: never cut behavior a contract guarantees.
- **Step 1 — Search for an existing home (mandatory before cataloging):**
  - Diff alone hides duplication. For each new function, type, interface, or constant, identify its wrapped dependency (client, encoding, enum, config, test fake) and search:
    1. Shared/common/util packages wrapping that dependency (`rg -l '<DependencyType>'`). If the shared home is missing one small piece, move that piece there instead of keeping a private copy.
    2. Same package and sibling packages.
    3. Existing helpers inside the test file itself.
    4. Platform features (stdlib, framework, DB, proxy).
- **Step 2 — 12-item cut catalog:**
  - Duplicate of existing code; pass-through constructor/wrapper; one-implementation abstraction; platform already does it; multiple guards compensating for one missing invariant (fix root cause once); unneeded config/flag/parameter knobs (make constant); duplication across units; dead code; copy-pasted test setup (3+ repeats → helper or table test); bypassed test helper; duplicate tests (merge union of assertions); stale docs/PR claims.
  - Not a trim: renames, formatting, style preferences, or contract-breaking cuts.
- **Step 3 — Report:**
  - Numbered most-lines-saved first (`### #N · <category> · \`path:lines\``), naming what to delete, what replaces it (exact existing symbol + path, or "nothing"), estimated lines saved, and a total at the end.
- **Step 4 — Apply (only with `apply`):**
  - Smallest blast radius first (move/extend shared code before switching callers).
  - Run verification + tests for every touched package.
  - Commit once listing each cut; never push.

## Worker
- In `apply` mode, run `@%UR_SKILL_HOOKS%/implement/subtask-verifications.md` (optional hook; fall back to `CLAUDE.md` commands if absent) plus tests for touched packages.

## Local
- In `apply` mode, run the project's documented verification/lint commands (`CLAUDE.md` / `AGENTS.md` / `Makefile`) plus tests for touched packages.

## Guardrails
- Never skip Step 1 codebase search ("diff looks self-contained" is a red flag).
- Never fix bugs or add defensive guards during a trim pass — report bugs for `code-review`.
- Never keep a private copy because a shared wrapper lacks one small method — add it to the shared wrapper.
- Never push commits; never edit files unless `apply` was passed.
