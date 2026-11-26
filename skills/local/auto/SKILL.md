---
name: auto
description: Use when asked to keep going autonomously on a multi-step build ("keep going", "work autonomously", "auto", "don't stop unless you must") or execute designed work end to end with risk-scaled independent review
---

# auto: Autonomous Work-and-Review Harness

You are the **orchestrator**. Never implement or review inline — drive `implement`, `code-review`, and `trim` through subagents, then triage via `receiving-code-review`, verify directly, and commit. Keep your own context small: read contracts, diffs, and reports, not whole codebases.

**Core principles:**
- Nothing counts until you verify it yourself; subagent claims are not evidence.
- A unit is done when it satisfies its **scope contract**, not when reviewers run out of ideas.
- Spend review passes where a missed defect is expensive.

## Autonomy Rules

- Work from an already-settled design (design doc, plan, prompt, or optional `ur` tickets). If no design exists yet, run `design` with the user **before** starting the loop.
- Once the loop starts, run through all units to completion without pausing for status check-ins, option menus, or non-blocking questions.
- Decide open questions yourself using the narrowest reading that satisfies the design, record each decision in your run log, and continue.
- Interrupt the user in chat **only** on a Hard stop (below) after finishing all other unblocked work.
- Do not push or open PRs unless asked.

## Driven Skills

Name the skill explicitly in every subagent prompt ("Invoke the `implement` skill ..."). Do NOT paraphrase a skill's instructions — pass only the contract, lens, owned paths, and required output.

| Stage | Skill | Runner |
| --- | --- | --- |
| Implement / apply fix batches | `implement` | Implementer subagent |
| Review pass (one lens, `N=1`) | `code-review` | Reviewer subagent |
| Triage reviewer findings | `receiving-code-review` | You |
| End-of-run cut pass | `trim` | Reviewer subagent |

Tell implementer subagents: *"work directly without dispatching nested subagents; do not push; if blocked return `BLOCKED: <reason>` instead of asking the user."* Override `receiving-code-review`'s "discuss with human" — decide against the contract and record why.

## Before Unit 1

Record `<run-base>` via `git rev-parse HEAD` for the end-of-run Trim pass.

## Unit Sizing (Batches)

A **unit** is a **batch** (~500–1500 non-test lines): one sizable task or a dependency-ordered group of related subtasks. This supersedes 1–3 file per-ticket granularity.
- Before breaking down a large scope (> ~3k lines), sanity-check against the cheapest reading of the ask (keep working systems, port only what's needed, drop doubtful work) and log the scope decision.
- Run **one batch at a time** through the full loop before moving to the next.

## Scope Contract (Write Before Code)

Write `tmp/<unit>-contract.md` and pass it **verbatim** to every implementer and reviewer:
- **Requirement:** the user's ask in their words.
- **Threat / failure model:** what we defend against, and explicitly what we do **not** defend against.
- **Delegated to platform:** what stdlib, framework, DB, OS, or proxy already provides.
- **Non-goals:** temptations reviewers must not add.
- **Size budget:** expected non-test lines (>2x triggers a cut batch).
- **Review depth:** `Default` or `Critical`.

Never widen scope on a reviewer's say-so; record wider ideas as follow-ups in the final report.

### Review Depth Tiers

| Tier | Applies to | Policy |
| --- | --- | --- |
| **Default** | Plumbing, config, refactors, UI, internal tooling | **1 pass.** Fix DEFECTs, verify, stop. Re-review only if a fix changed behavior non-trivially. |
| **Critical** | Auth, security boundaries, credentials, sandboxing/permissions, crypto, untrusted-input parsing, prod data loss/migrations, public API contracts (regardless of small size) | New lens per pass, strongest model for reviewers. After triage, classify pass as **minor** (nits, out-of-contract, test gaps, doc fixes, small local fix → apply, verify, **stop**) or **major** (non-local fix, new machinery, threat-surface redesign → apply, verify, next pass with new lens). **Hard cap: 3 passes** (at cap: fix last pass's DEFECTs, verify, record remaining as follow-ups, move on). |

## Per-Batch Loop

1. Write `tmp/<unit>-contract.md` (with review depth).
2. Dispatch implementer subagent invoking `implement` (no nested subagents, no push) with contract + owned paths. Reuse this **same** implementer for fix batches.
3. Check non-test diff size against budget; >2x triggers an immediate cut batch before review.
4. Dispatch **one** reviewer subagent (`code-review`, sequential `N=1`, never parallel) on `<base>..<head>` with the contract and **one lens** matching the threat model:
   - Read-only on repo (probes in scratch dir).
   - Run a probe for every claim and mark `confirmed: yes/no`.
   - Tag every finding `DEFECT` | `OUT-OF-CONTRACT` | `NIT`.
   - Findings and verdict only (no fixes).
5. Triage yourself with `receiving-code-review` against the contract (P0–P3 is not contract class). Resolve via **code**, **documented known limit**, or **rejection** (cheapest that satisfies contract). Write the accepted fix batch to `tmp/<unit>-batch<N>.md`.
6. Send the fix batch to the **same** implementer; require every new test to fail when its fix is reverted, and commit per batch.
7. Verify directly yourself: run the repo's documented build/test/lint commands (`CLAUDE.md` / `AGENTS.md` / `Makefile`), `git status`, and size vs budget.
8. Decide re-review per tier, close the unit (and close any `ur` tickets if used), record decisions/known limits, and move to the next batch.

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
| Non-test code >2x size budget | Cut batch (no review) back to platform-native minimum; log decision |
| Fix re-implements platform feature | Use platform feature or mark `OUT-OF-CONTRACT` |
| Speculative machinery (new preflight/enum/builder) | Drop unless required by design; record known limit or follow-up |
| Same surface has DEFECTs 3 passes straight | Redo surface from contract, then 1 `Fresh read` pass |
| Implementer returns `BLOCKED:` | Settle from design/codebase (narrowest reading), send decision back |
| Buy-vs-build | Prefer existing repo dep/platform; only add third-party dep if design names it |

## End of Run: Single Trim Pass & Final Report

Run **once** after the last unit closes (never per unit):
1. Reviewer subagent invokes `trim` (report-only, no `apply`) on `<run-base>..HEAD` with all unit contracts.
2. Triage findings → send accepted cuts to the most recent implementer (or a fresh implementer).
3. Verify directly with project build/test/lint commands and commit. Trim never triggers another review pass.
4. Present the final report in chat: units completed, review tiers/passes, decisions made, review caps hit, follow-ups, and known limits.

## Hard Stops (The Only Ones)

Stop and ask the user in chat **only** after finishing all unblocked units when:
- An unauthorized action is **irreversible or destructive** to data or shared state (local commits/tests never qualify).
- **Credentials, access, or infrastructure** are missing and no other work remains.
- **Design contradicts itself or the codebase** so severely that every reading builds the wrong thing.

Present one concrete question with your recommended answer.

## Red Flags

- Implementing or reviewing inline instead of dispatching subagents
- Paraphrasing a skill into a subagent prompt instead of naming it
- Running parallel reviewers (`N > 1`) or spawning a fresh implementer for fixes
- Trusting subagent test claims without running verification yourself
- Treating P0–P3 as contract class, running off-contract lenses, exceeding 3 passes, or re-reviewing after a minor pass or Trim

$ARGUMENTS
<!-- compiled-from: skills/specs/auto.md@128e83f7d046 profile:local@daa4419e0589 -->
