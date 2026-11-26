---
name: code-review
description: Use when reviewing a proposed code change, diff, branch, or pull request for bugs, design soundness, and test quality, optionally with N independent parallel reviewers
---

# Code Review

Review a proposed code change made by another engineer. Perform **two distinct jobs** and never blur them:
- **Bug hunt** — defects in the diff: wrong behavior, crashes, security issues, performance regressions, and maintainability defects (dead code, misleading docs, weak tests).
- **Design review** — soundness of the approach: whether contracts, abstractions, boundaries, consistency, and extensibility fit their purpose and consumers even when the code is correct and tests pass.

Never dress a design concern up as a bug, and never soften a real bug into a nit. More specific guidelines in a developer/user prompt or project rules below override these defaults.

## Optional Hook Files

The `@%UR_SKILL_HOOKS%/...` reference below is an optional project hook. Read and follow it if the file exists. If absent, skip it silently and continue — absence is never a blocker, never a reason to call `request-human`, and never a reason to create missing hooks or invent replacement rules.

## Invocation & Parallel Review

Parse `$ARGUMENTS` for an optional positive integer `N` (default `1`):
- **`N = 1` (or omitted):** perform the review directly.
- **`N > 1`:** launch `N` independent reviewer subagents concurrently in one batch with identical prompts pointing at the same diff/branch/PR, instructing each to follow this skill in full without coordinating and return the prose Output Format below.

When combining `N > 1` reports, merge into a single review (do not concatenate):
- Deduplicate findings sharing the same root cause and location (keep clearest summary, union distinct details).
- Union unique findings reported by only one subagent.
- Re-filter merged findings against existing PR comments so no duplicates slip through.
- Take the highest priority when merged findings disagree.
- Issue one verdict: `incorrect` if *any* subagent surfaced a confirmed blocking bug (re-verify disagreements yourself rather than voting). Renumber findings sequentially.

## Mandatory Pre-Review Reading

Complete these reads before judging the diff:
1. **Referenced definitions:** read types, constants, interfaces, DB schema, migrations, and protos the diff references.
2. **Non-trivial callees:** read implementations and all conditional branches called by the diff (stop at 2 hops unless pulled deeper).
3. **Trace `nil` / zero values:** follow every `nil` or zero-value argument passed to a constructor or function through storage to every reachable dereference.
4. **Construction sites:** when a new DI/struct field is added, search all `Type{...}` literals to verify wiring and trace whether any intentionally-nil field is reachable.
5. **Branch coverage in tests:** check that tests exercise every new runtime branch (flags, config, network/mode switches).
6. **Intent and larger effort:** read the PR description, design docs, and parent/sibling/follow-up tickets to identify multi-step plans and known future consumers.
7. **Incumbent comparison:** when replacing or absorbing existing functionality, enumerate every output the incumbent produced (return values, side effects, metrics, events, persisted data) and check the new contract against it.
8. **Skim existing PR comments once:** use them as input only — never duplicate an already-raised finding; inspect anything they point to; do not reply, summarize, or track status (that belongs to the `address-feedback` skill).
9. **Verify doc/comment/PR claims:** check every claim that a type satisfies an interface, a guarantee holds, or the PR implements a check/limit against the actual code.
10. **Search for an existing home (no 2-hop limit):** for each new helper, interface, wrapper, or builder wrapping a dependency, search the repo (`rg -l '<DependencyType>'`) for an existing shared package that already wraps it.

## Bug Criteria

Flag as `bug` when introduced in this commit, discrete/actionable, appropriate to repo rigor, provably affecting accuracy/integrity/concurrency/security/performance/maintainability, and not clearly intentional:
- **Reachable nil derefs from intentional nil wiring are bugs** — trace and show the full call chain from construction site to dereference.
- **Framework semantic mismatches** (e.g., timeout/retry placed on a construct that ignores it) creating silent unbounded waits or missing safety nets are bugs.
- **Documented-required but unvalidated fields/options** at a boundary are findings (P2/P3) even if the only current caller passes a valid value.
- **Risky pre-existing patterns re-instantiated at a new boundary** are in scope (P2/P3; explicitly note it mirrors existing code and is not a regression).

## Design Review Criteria

Flag as `design` only when you can name the **specific consumer or scenario**, the **concrete inadequacy**, and the **decision to make now** (no bikeshedding or vague unease):
1. **Contract adequacy** *(highest value)* — replacement drops an incumbent output, signature cannot carry what a planned caller needs, callback lacks required context.
2. **Abstraction fit** — layer leaks, speculative generalization, duplicates an existing shared abstraction, or wrong layer.
3. **Consistency** — introduces a second pattern without stated reason where the codebase has a standard one.
4. **Extensibility traps** — choice that forces a breaking change, migration, or painful refactor on a near-certain next step.

Frame design findings as forward-looking decisions ("current code is correct; worth deciding while cheap"), typically P2/P3 non-blocking unless blocking the change's stated goal.

## Test Quality (P2/P3)

Flag weak tests that add cost without protecting real logic:
- **Tautological / change-detector tests** (stubs return constants and test asserts the same constant, or assertions cannot fail).
- **Redundant tests** hitting identical paths with different constants instead of distinct branches/boundaries.
- **Copy-pasted setup** repeated 3+ times (suggest a fixture helper or table test; name repeated lines and locations).
- **Bypassed test helpers** (inline duplication of an existing helper).
- **Duplicate tests** sharing setup/path where one only adds an assertion (merge union of assertions).
- **Over-mocking** replacing the collaborator holding the real logic so only delegation is tested.

## Additional Quality Checks

Output **all** qualifying findings — do not stop at the first, and do not suppress genuine non-blocking hardening/defense-in-depth findings (exception: skip purely defensive nil-guards on unreachable constructor-wired values). Ignore trivial style unless it obscures meaning or breaks documented standards.
- Flag introduced `TODO`, `FIXME`, `XXX`, or `HACK` comments (include full comment text).
- Flag dead/orphaned code introduced or orphaned by the diff (confirm no re-exports, reflection, codegen, or string dispatch; ignore intentional public API or dead-code-suppressed items; P2/P3).
- Flag pass-through constructors/wrappers that only copy params into a struct literal or forward one call unchanged (P3; skip sole exported builder for an unexported type).
- Flag overly wide positional tuple returns where a named struct/record fits (P3).
- Flag hardcoded literals duplicating an existing named constant or enum (P3).
- Flag loop-invariant computations/validations placed inside loops (P3, higher if costly).

## Output Format

Present as **prose only** (never JSON). Do NOT generate a PR fix or edit files. Use a matter-of-fact, concise tone with zero flattery ("Great job", "Thanks").

### Findings

Order sequentially, most-severe first (`P0` drop-everything universal blocker · `P1` urgent · `P2` normal · `P3` low/nice-to-have). Separate findings with `---`. Every finding has exactly 4 distinct parts:
1. **Header:** `### #N · PN · bug|design · \`path/to/file:start-end\`` (short range ≤5–10 lines; do not repeat location in body).
2. **Headline:** one standalone bold sentence naming the defect or decision.
3. **Impact:** single-paragraph blockquote (`> **Impact** — ...`) stating what breaks, preconditions, and reachability (for `design`: state `none today; the current code is correct` and name the decision).
4. **Details:** 1–2 concise paragraphs of evidence (call chain, definitions, consumer gap) and fix direction. Code chunks ≤3 lines; use ```` ```suggestion ```` ONLY for minimal concrete replacements preserving exact leading whitespace/indentation.

```markdown
### #1 · P1 · bug · `path/to/file.go:154-160`

**One bold sentence naming the defect.**

> **Impact** — what breaks, for whom, under what preconditions, and whether any path reaches it today.

Evidence, call chain, definitions, and fix direction.

---

### #2 · P3 · design · `path/to/other.go:47-62`

**One bold sentence naming the decision at stake.**

> **Impact** — none today; the current code is correct. Name the decision and why it is cheap now.

Evidence and the specific consumer or scenario the current shape cannot serve.
```

### Verdict

End with `## Verdict: correct` or `## Verdict: incorrect` followed by 1–3 sentences of justification. Design findings alone do not make a patch `incorrect` unless one blocks the change's stated goal (note when verdict is `correct` with design findings).

### Disagreements *(Optional)*

Include ONLY when actively disagreeing with an existing PR comment (wrong about code, harmful if applied, or already resolved). Omit completely when agreeing with everything.

```markdown
### <person> — <topic in a few words>

**Disagree:** why the comment is wrong or resolved, with evidence.
```

## Project Rules

@%UR_SKILL_HOOKS%/code-review/review-guidelines.md

$ARGUMENTS
<!-- compiled-from: skills/specs/code-review.md@326eed11fbc1 profile:worker@6afed0c52191 -->
