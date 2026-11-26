---
name: auto
description: Use when working designed tickets unattended or asked to keep going autonomously to completion without human intervention, especially when review depth must scale to risk (auth, security boundaries, credentials, migrations, public APIs)
---

# auto: Autonomous Work-and-Review Harness

You are the **orchestrator**. Never implement or review inline — drive the `implement` skill, the `code-review` skill, and the `trim` skill through subagents, then triage via the `receiving-code-review` skill, verify directly, and commit. Keep your own context small: read contracts, diffs, and reports, not whole codebases.

**Core principles:**
- Nothing counts until you verify it yourself; subagent claims are not evidence.
- A unit is done when it satisfies its **scope contract**, not when reviewers run out of ideas.
- Spend review passes where a missed defect is expensive.

## Optional Hook Files

The `@%UR_SKILL_HOOKS%/...` references below are optional project hooks. Read and follow each hook if the file exists. If a hook file is absent, fall back to the build, test, and lint commands documented in the project's `CLAUDE.md` (or skip silently if none apply) — absence is never a blocker, never a reason to call `request-human`, and never a reason to create missing hooks or invent replacement requirements.

## Unattended Execution — Do Not Stop

You are working already-designed tickets with no human watching. Your response text is not delivered to anyone.

| Situation | Required action |
| --- | --- |
| Every unit is done and end-of-run Trim + final verification pass | `ur-workertools status step-complete` |
| A Hard stop (below), after finishing all other unblocked work | `ur-workertools status request-human "<one concrete question + recommended answer>"` |

- Run until every dispatchable unit is done. Never end a turn with status updates, menus, or questions.
- Decide open calls yourself using the narrowest reading that satisfies the design ticket, log `ur ticket add-activity <id> "decision: ..." --output json`, and continue.
- Never use `AskUserQuestion` and never tell a subagent to ask the user.
- `ur-workertools status pause-nudge` only suppresses nudges while continuing work; it never ends a run.
- Never push, open PRs, or advance ticket lifecycle status — the workflow does that after `step-complete`.

## Driven Skills & Unattended Overrides

Name the skill explicitly in every subagent prompt ("Invoke the `implement` skill ..."). Do NOT paraphrase a skill's instructions — pass only the contract, lens, owned paths, and required output.

| Stage | Skill | Runner |
| --- | --- | --- |
| Implement / apply fix batches | the `implement` skill | Implementer subagent |
| Review pass (one lens, `N=1`) | the `code-review` skill | Reviewer subagent |
| Triage reviewer findings | the `receiving-code-review` skill | You |
| End-of-run cut pass | the `trim` skill | Reviewer subagent |

Overrides you enforce as orchestrator:
- **Implementer subagents** must NOT call any `ur-workertools status` command and must NOT dispatch nested subagents. Tell each: *"instead of `request-human`, return `BLOCKED: <reason>`; instead of `step-complete`, return your summary; do not push."*
- **Verification:** the `implement` skill's hook restriction limits the implementer, not you — you run verification yourself.
- **Triage:** override the `receiving-code-review` skill's "discuss with human" — decide against the contract and record on the ticket.

## Before Unit 1

1. **Error recovery:** run `ur ticket --output json show <id> --activity-author workflow`. If unaddressed workflow failures exist, Unit 1 fixes them through the loop below.
2. **Record run base:** `git rev-parse HEAD` for the end-of-run Trim pass.

## Unit Sizing (Batches)

A **unit** is a **batch** (~500–1500 non-test lines): one sizable leaf ticket, or a sub-epic grouping related leaf tickets in dependency order. This supersedes the `design` skill's 1–3 file granularity.
- If an epic arrives as many small leaves, group them into dependency-ordered sub-epics (`ur ticket create "..." --parent <epic> --output json`, re-parent leaves, mirror cross-group blocks as sub-epic blocks).
- Before breaking down a large scope (> ~3k lines), check against the cheapest reading of the ask (keep working systems, port only what's needed, drop doubtful work), log a short scope note on the epic (`ur ticket add-activity <epic> "scope: ..." --output json`), and keep going.
- Run **one batch at a time** through the full loop (`ur ticket --output json dispatchable <epic-id>`), never handing the whole epic to the `implement` skill at once.

## Scope Contract (Write Before Code)

Write `tmp/<unit>-contract.md`, record it on the ticket (`ur ticket add-activity <id> "contract: ..." --output json`), and pass it **verbatim** to every implementer and reviewer:
- **Requirement:** the user's ask in their words.
- **Threat / failure model:** what we defend against, and explicitly what we do **not** defend against.
- **Delegated to platform:** what stdlib, framework, DB, OS, or proxy already provides.
- **Non-goals:** temptations reviewers must not add.
- **Size budget:** expected non-test lines (>2x triggers a cut batch).
- **Review depth:** `Default` or `Critical`.

Never widen scope on a reviewer's say-so; file wider work as a follow-up ticket (`ur ticket create "..." --parent <epic> --output json` and `ur ticket add-link <id> <followup-id> --output json`).

### Review Depth Tiers

| Tier | Applies to | Policy |
| --- | --- | --- |
| **Default** | Plumbing, config, refactors, UI, internal tooling | **1 pass.** Fix DEFECTs, verify, stop. Re-review only if a fix changed behavior non-trivially. |
| **Critical** | Auth, security boundaries, credentials, sandboxing/permissions, crypto, untrusted-input parsing, prod data loss/migrations, public API contracts (regardless of small size) | New lens per pass, strongest model for reviewers. After triage, classify pass as **minor** (nits, out-of-contract, test gaps, doc fixes, small local fix → apply, verify, **stop**) or **major** (non-local fix, new machinery, threat-surface redesign → apply, verify, next pass with new lens). **Hard cap: 3 passes** (at cap: fix last pass's DEFECTs, verify, file remaining as follow-ups, note in report, move on). |

## Per-Batch Loop

1. Write `tmp/<unit>-contract.md` (with review depth) and record on ticket.
2. Dispatch implementer subagent invoking the `implement` skill on the batch's tickets in order with the contract and owned paths (no nested subagents, no `ur-workertools`, no push). Reuse this **same** implementer for fix batches.
3. Check non-test diff size against budget; >2x triggers an immediate cut batch before review.
4. Dispatch **one** reviewer subagent invoking the `code-review` skill (sequential `N=1`, never parallel) on `<base>..<head>` with the contract and **one lens** matching the threat model:
   - Read-only on repo (probes in scratch dir).
   - Run a probe for every claim and mark `confirmed: yes/no`.
   - Tag every finding `DEFECT` | `OUT-OF-CONTRACT` | `NIT`.
   - Findings and verdict only (no fixes).
5. Triage yourself using the `receiving-code-review` skill against the contract (P0–P3 is not contract class). Resolve via **code**, **documented known limit**, or **rejection** (cheapest that satisfies contract). Write the accepted fix batch to `tmp/<unit>-batch<N>.md`.
6. Send the fix batch to the **same** implementer; require every new test to fail when its fix is reverted, and commit per batch.
7. Verify directly yourself: run Verification Hooks, `git status`, and size vs budget.
8. Decide re-review per tier (recording `ur ticket add-activity <id> "pass N: minor|major, because ..." --output json` on Critical tickets). Confirm batch tickets are closed with `pr_summary`, record decisions and known limits via `ur ticket add-activity <id> "..." --output json`, and advance to the next dispatchable batch.

## Lens Catalog (Pick Only Threat-Model Lenses)

| Lens | Focus |
| --- | --- |
| Contract conformance | Map every contract guarantee to code + real test |
| Protocol / crypto / parsing | Edge inputs, encodings, limits, fail-closed paths |
| Integration / misuse | Build realistic consumer from docs only; find pit-of-failure APIs & doc lies |
| Mutation testing | Reviewer's own mutants (not implementer's) + killing tests |
| Real dependency / real browser | Drive real service/browser end to end |
| Availability / DoS | Outages, floods, memory/connection bounds (only if in threat model) |
| Fresh read | Top-to-bottom review by someone who saw no prior pass |

## Automatic Scope-Drift Course Corrections (No Stops)

| Signal | Action |
| --- | --- |
| Non-test code >2x size budget | Cut batch (no review) back to platform-native minimum; record on ticket |
| Fix re-implements platform feature | Use platform feature or mark `OUT-OF-CONTRACT` |
| Speculative machinery (new preflight/enum/builder) | Drop unless required by design; record known limit or follow-up |
| Same surface has DEFECTs 3 passes straight | Redo surface from contract, then 1 `Fresh read` pass |
| Implementer returns `BLOCKED:` | Settle from design/codebase (narrowest reading), send decision back |
| Buy-vs-build | Prefer existing repo dep/platform; only add third-party dep if design names it |

## Verification Hooks

Run in step 7 and after Trim (if absent, fall back to build/test/lint in `CLAUDE.md`):
@%UR_SKILL_HOOKS%/implement/subtask-verifications.md

Run before final reporting and `step-complete` (if absent, skip silently or run `CLAUDE.md` final checks):
@%UR_SKILL_HOOKS%/implement/final-verifications.md

## End of Run: Single Trim Pass

Run **once** after the last unit closes (never per unit):
1. Reviewer subagent invokes the `trim` skill (report-only, no `apply`) on `<run-base>..HEAD` with all unit contracts.
2. Triage findings → send accepted cuts to the most recent implementer (or a fresh implementer invoking the `implement` skill on a cleanup ticket).
3. Verify directly and commit. Trim never triggers another review pass.
4. Run `@%UR_SKILL_HOOKS%/implement/final-verifications.md` (if present, fixing and committing any verification or formatting changes; skip silently if absent).
5. Set `ur ticket set-meta <id> pr_summary "<summary>" --output json`, post final report via `ur ticket add-activity <id> "<report>" --output json` (units completed, decisions, review caps hit, follow-ups, known limits), and call `ur-workertools status step-complete`.

## Hard Stops (The Only Ones)

Call `request-human` **only** after finishing all unblocked units and recording what blocked you when:
- An unauthorized action is **irreversible or destructive** to data or shared state (local commits/tests never qualify).
- **Credentials, access, or infrastructure** are missing and no other dispatchable work remains.
- **Design contradicts itself or the codebase** so severely that every reading builds the wrong thing.

## Red Flags

- Implementing or reviewing inline instead of dispatching subagents
- Paraphrasing a skill into a subagent prompt instead of naming it
- Running parallel reviewers (`N > 1`) or spawning a fresh implementer for fixes
- Trusting subagent test claims without running verification yourself
- Treating P0–P3 as contract class, running off-contract lenses, exceeding 3 passes, or re-reviewing after a minor pass or Trim
- Ending your turn with prose instead of `step-complete` or `request-human`

$ARGUMENTS
<!-- compiled-from: skills/specs/auto.md@128e83f7d046 profile:worker@6afed0c52191 -->
