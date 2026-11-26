---
name: trim
description: Use when a change is functionally complete and before it ships to find and cut duplicated, wrapper, dead, or unnecessary code
---

# trim: Make the Diff Smaller

Inspect a finished diff with one question: **what can go?** Cut code by deleting it, replacing it with code the repo or platform already provides, or folding copies into one. Never hunt bugs or add guards — use the `code-review` skill for that.

## Optional Hook Files

The `@%UR_SKILL_HOOKS%/...` reference below is an optional project hook. Read and follow it if the file exists. If absent, fall back to the verification commands documented in `CLAUDE.md` (or skip silently if none apply) — absence is never a blocker, never a reason to call `request-human`, and never a reason to create missing hooks or invent replacement checks.

## Invocation

Parse `$ARGUMENTS` for:
- A range (`<base>..<head>`, PR number, or branch; default: working branch vs merge base with default branch).
- `apply`: perform accepted cuts, verify, and commit. Without `apply`, **report only** (do not edit files).
- Scope contracts (if provided): never cut behavior a contract guarantees.

## Step 1: Search for an Existing Home (Required)

The diff alone hides duplication of code elsewhere. For every new function, type, interface, or constant, identify what dependency it wraps (client, encoding, enum, config, test fake) and search:
1. **Shared packages** (`rg -l '<DependencyType>'`): read the packages importing it most. If an existing shared wrapper lacks one small piece, **add it there and use it** instead of keeping a private copy.
2. **Same and sibling packages:** check neighboring files for existing helpers.
3. **The test file itself:** check for existing helpers bypassed by inline copies.
4. **The platform:** stdlib, framework, DB, or proxy behavior re-implemented in the diff.

## Step 2: Cut Catalog

| Category | Look for | Usual cut |
| --- | --- | --- |
| Duplicate of existing code | Step 1 hits matching an existing wrapper/helper/builder | Use existing home; move missing piece there |
| Pass-through constructor/wrapper | Body only copies params into a struct literal or forwards one call | Inline at callers; tests build literal |
| One-implementation abstraction | Interface/option/strategy with one impl and no planned second | Use concrete type |
| Platform already does it | Re-implemented stdlib, framework, DB, or proxy feature | Delete and rely on platform |
| Guards for one root cause | Multiple checks compensating for one missing invariant | Fix root cause once; drop guards |
| Unneeded knobs | Config fields, flags, or params with one value in practice | Replace with constant |
| Duplication across units | Same logic in two files or units of the change | Single implementation called by both |
| Dead code | Unreachable symbols, branches, imports, or params | Delete |
| Copy-pasted test setup | Same arrange/prelude repeated 3+ times | Fixture helper or table test |
| Bypassed test helper | Existing helper bypassed by inline copies | Use the helper everywhere |
| Duplicate tests | Same setup and path where one adds only an assertion | Merge keeping union of assertions |
| Stale docs | Doc comments, READMEs, or PR claims no longer true | Delete or correct |

**Not a trim:** renames, reformatting, style preferences, or any cut that breaks a guaranteed contract.

## Step 3: Report

Number findings sequentially, **most lines saved first**:

```markdown
### #1 · duplicate · `path/to/client.go:23-40`

**`fooClient` and its private interface duplicate `path/to/common/foo`.**

Add the missing method to the shared wrapper, hold it directly, and delete the private interface and builder. Saves ~40 lines.
```

Name the code to remove, its exact replacement (`symbol` + path, or "nothing"), and estimated lines saved. End with the estimated total lines cut.

## Step 4: Apply (Only With `apply`)

1. Make cuts smallest blast radius first (move/extend shared code before switching callers).
2. Run verification (hook below or `CLAUDE.md` commands) plus tests for every touched package:
@%UR_SKILL_HOOKS%/implement/subtask-verifications.md
3. Commit once listing each cut. Never push.

## Red Flags

- Skipping Step 1 because "the diff looks self-contained"
- Keeping a private copy because the shared wrapper lacks one small method
- Fixing bugs or adding guards during a trim pass (report them for the `code-review` skill)
- Editing files when `apply` was not passed, or pushing commits

$ARGUMENTS
<!-- compiled-from: skills/specs/trim.md@cc1264b75a41 profile:worker@6afed0c52191 -->
