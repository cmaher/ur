# code-review

- **Targets:** worker, local
- **Trigger:** Reviewing a proposed code change (diff, branch, or PR) for bugs, design soundness, and test quality, optionally with `N` independent parallel reviewer subagents.

## Core
- **Two distinct jobs (do both, never blur them):**
  - **Bug hunt:** defects in the diff — wrong behavior, crashes, security, performance regressions, maintainability defects (dead code, misleading docs, weak tests).
  - **Design review:** soundness of the approach — contracts, abstractions, boundaries, consistency, extensibility — even when code is correct and tests pass.
  - Never dress a design concern as a bug or soften a real bug into a nit.
- **Invocation & parallel review (`N`):**
  - Parse optional positive integer `N` (default `1`).
  - `N = 1`: review directly.
  - `N > 1`: launch `N` independent subagents concurrently with identical prompts (same diff, full guidelines, no coordination). Merge into one report: deduplicate same root cause/location (union distinct details), union unique findings, re-filter against existing PR comments, take highest priority on conflict, single verdict (incorrect if *any* confirmed blocking bug — re-verify disagreements yourself rather than voting).
- **Mandatory Pre-Review Reading (before judging the diff):**
  1. Referenced definitions (types, constants, interfaces, DB schema, migrations, protos).
  2. Non-trivial callees and all conditional branches (up to 2 hops unless pulled deeper).
  3. Trace `nil`/zero-value arguments through storage to every potential dereference.
  4. All construction sites (`Type{...}`) for newly added DI/struct fields.
  5. Test coverage of every new runtime branch (flags, config, network/mode switches).
  6. Change intent & larger effort (PR description, design docs, parent/sibling/follow-up tickets, known future consumers).
  7. Incumbent comparison when replacing/absorbing existing functionality (enumerate all outputs, side effects, metrics, events, persisted data).
  8. Skim existing PR review comments once as input (do not duplicate raised findings; inspect anything they point to; do not reply or track status).
  9. Verify claims in comments, docstrings, and PR descriptions against actual signatures/behavior.
  10. Search codebase (no 2-hop limit) for an existing shared home/wrapper for any new dependency-wrapping helper, interface, or builder.
- **Bug criteria:**
  - Introduced in the commit, discrete/actionable, matches repo rigor, provably affects behavior/integrity/concurrency/security/performance/maintainability, not clearly intentional.
  - Always flag: reachable nil derefs from intentionally-nil wiring (show call chain); framework semantic mismatches creating silent unbounded waits/ignored bounds; fields documented as required but never validated at the boundary; risky pre-existing patterns re-instantiated at a *new* boundary (note it mirrors existing code, P2/P3).
- **Design review criteria (must name concrete consumer/scenario + inadequacy + decision; no bikeshedding):**
  - **Contract adequacy** (highest value — e.g. replacement dropping incumbent output, signature blocking planned caller), **Abstraction fit** (leaks, speculative, duplicates shared abstraction, wrong layer), **Consistency** (unjustified second pattern), **Extensibility traps** (breaks near-certain next step).
  - Frame as forward-looking decision ("current code is correct"); typically P2/P3 non-blocking unless it blocks the stated goal.
- **Test quality findings (P2/P3):**
  - Tautological/change-detector tests, redundant identical-path tests, copy-pasted setup (3+ occurrences), bypassed existing test helpers, duplicate tests differing by one assertion, over-mocking that only tests delegation.
- **Additional quality checks:**
  - Flag introduced `TODO`/`FIXME`/`XXX`/`HACK` (include full text); dead/orphaned code (confirm no re-exports/reflection/codegen/string dispatch); pass-through constructors/wrappers (except sole exported builder for unexported type); overly wide positional tuple returns; hardcoded literals duplicating existing constants/enums; loop-invariant checks inside loops.
  - Defense-in-depth findings are welcome (except purely defensive nil-guards on unreachable constructor-wired values). Ignore trivial style unless it obscures meaning or breaks documented standards.
- **Output format (prose, NEVER JSON; do NOT generate a PR fix):**
  - Order findings sequentially, most-severe first (`P0`–`P3`), category `bug` or `design`.
  - Strict 4-part block per finding separated by `---`:
    1. Header: `### #N · PN · bug|design · \`path:lines\`` (short range ≤5–10 lines; don't repeat path in body)
    2. Headline: one standalone bold sentence
    3. Impact: blockquote (`> **Impact** — ...`) stating what breaks, preconditions, reachability (or "none today; current code is correct" for design)
    4. Details: 1–2 concise paragraphs of evidence/call-chain/fix direction; code blocks ≤3 lines; `suggestion` blocks only for minimal concrete replacements preserving exact indentation.
  - `## Verdict: correct` or `## Verdict: incorrect` + 1–3 sentences justification (design findings alone don't flip verdict unless blocking stated goal).
  - Optional `### Disagreements` section ONLY when actively disagreeing with an existing PR comment (omit if none).
  - Tone: matter-of-fact, concise, zero flattery ("Great job", "Thanks").

## Worker
- Include optional project review rules hook under `## Project Rules`: `@%UR_SKILL_HOOKS%/code-review/review-guidelines.md` (skip silently if absent).

## Local
- No `@%UR_SKILL_HOOKS%`; check repo `CLAUDE.md`/`AGENTS.md` or any user-provided review guidelines instead.

## Guardrails
- Never skip Pre-Review Reading or review from the diff alone.
- Never duplicate an existing PR comment in Findings (only use `Disagreements` if actively disagreeing).
- Never output JSON, never modify files / generate PR fixes, never use flattery or >3-line code chunks in comments.
- Never silently drop non-blocking quality/hardening findings; never bikeshed personal preferences as design findings.
