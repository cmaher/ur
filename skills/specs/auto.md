# auto

- **Targets:** worker, local
- **Trigger:** Asked to build/work autonomously to completion ("keep going", "work autonomously", "auto", "don't stop unless you must"), unattended implementation of designed work, or contract-bounded implement → review → trim execution with risk-scaled review depth.

## Core
- **Role — orchestrator only:** never implement or review inline. Drive `implement`, `code-review`, `receiving-code-review`, and `trim` through subagents, then triage, verify, and commit. Keep orchestrator context small (read diffs and reports, not whole codebases).
- **Core principles:** nothing counts until the orchestrator verifies it directly; a unit is done when it satisfies its **scope contract**, not when reviewers run out of ideas; spend review time where defects are expensive.
- **Unit sizing = batches (~500–1500 non-test lines):**
  - Supersedes `design`'s 1–3 file granularity — group related small tasks/leaves into coherent batches (dependency-ordered DAG) so each loop pays for itself.
  - Process **one batch at a time** through the full loop (contract → implement → review → fix → verify → close) before moving to the next.
  - Before breaking down a large scope (> ~3k lines), sanity-check against the cheapest reading of the ask (keep working systems, port only what's needed, drop doubtful work).
- **Scope contract (written to disk before code, e.g. `tmp/<unit>-contract.md`):**
  - Given **verbatim** to every implementer and reviewer.
  - Fields: `Requirement`, `Threat/failure model` (explicitly naming what we do NOT defend against), `Delegated to platform`, `Non-goals`, `Size budget` (expected non-test lines; >2x triggers cut), `Review depth` (`Default` or `Critical`).
  - Ambiguous open questions: pick the narrowest reading that satisfies the design, record the decision, and continue. Never widen scope on a reviewer's say-so (log as follow-up).
- **Review depth tiers:**
  - **Default** (plumbing, config, refactors, UI, internal tooling): **1 pass**. Fix DEFECTs, verify, stop. Re-review only if a fix changed behavior non-trivially.
  - **Critical** (auth, security boundaries, credentials, sandboxing/permissions, crypto, untrusted-input parsing, prod data loss/migrations, public API contracts — regardless of small size): new lens per pass, strongest model for reviewers. After triage, classify pass as **minor** (nits, out-of-contract, test gaps, doc fixes, small local defect fix → apply, verify, **stop reviewing**) or **major** (non-local fix / new machinery / threat-surface redesign → apply, verify, next pass with new lens). **Hard cap: 3 passes** (at cap: fix last pass's DEFECTs, verify, file remaining as follow-ups, note in report, move on).
- **Per-batch loop:**
  1. Write scope contract (including review depth).
  2. Dispatch implementer subagent invoking `implement` (no nested subagents) with contract + owned paths + "do not push". Reuse the **same** implementer for fix batches.
  3. Check non-test line count vs size budget (>2x → cut batch before review).
  4. Dispatch **one** reviewer subagent at a time (`code-review`, sequential `N=1`, never parallel) with contract + one lens from catalog: read-only on repo (probes in scratch dir), run a probe for every claim (`confirmed yes/no`), tag each finding `DEFECT` | `OUT-OF-CONTRACT` | `NIT`, findings + verdict only (no fixes).
  5. Triage findings yourself via `receiving-code-review` against the contract (P0–P3 ≠ contract class). Resolve via **code**, **documented known limit**, or **rejection** (prefer cheapest that satisfies contract). Write fix batch to disk (`tmp/<unit>-batch<N>.md`).
  6. Send fix batch to the SAME implementer; require every new test to fail when its fix is reverted; commit per batch.
  7. Verify directly yourself (tests/lint/`git status`/size budget — subagent claims are not evidence).
  8. Decide re-review per tier, close unit, proceed to next batch.
- **Lens catalog (choose only lenses matching the threat model):**
  - `Contract conformance`, `Protocol / crypto / parsing`, `Integration / misuse`, `Mutation testing` (reviewer's own mutants), `Real dependency / real browser`, `Availability / DoS` (only if in threat model), `Fresh read`.
- **Automatic scope-drift course corrections (decide, record, continue — do not stop):**
  - >2x size budget → cut batch to platform-native minimum (no review pass).
  - Fix re-implements platform feature → use platform or mark `OUT-OF-CONTRACT`.
  - Unit sprouts speculative machinery → drop unless required; record known limit/follow-up.
  - Same surface has DEFECTs 3 passes in a row → redo surface from contract + one `Fresh read` pass.
  - Implementer returns `BLOCKED:` → settle from design/codebase (narrowest reading), send decision back.
  - Buy-vs-build → prefer existing repo dep/platform; only add third-party dep if design names it.
- **End-of-run Trim pass (once per run, never per unit):**
  - Record `<run-base>` (`git rev-parse HEAD`) before unit 1.
  - After last unit closes, run reviewer subagent invoking `trim` (report-only) on `<run-base>..HEAD` with all contracts → triage → apply accepted cuts via implementer → verify & commit. Trim never triggers another review pass.
- **Hard stops (the ONLY valid reasons to interrupt/stop early):**
  - Unauthorized irreversible/destructive action on data or shared state (never local commits/tests).
  - Missing credentials/access/infra with no other dispatchable work remaining.
  - Design contradicts itself or codebase so badly every reading builds the wrong thing (finish all unblocked units first).

## Worker
- **Unattended stop contract:** follow `_profiles/worker.md`. End ONLY via `ur-workertools status step-complete` (after final verification + Trim) or `ur-workertools status request-human "<one concrete question + recommendation>"` (only on Hard stops, after finishing all other unblocked work).
- **Error recovery:** check `ur ticket --output json show <id> --activity-author workflow` before unit 1; if unaddressed failures exist, first unit fixes them.
- **Ticket mechanics:** group leaf tickets into sub-epic batches (`ur ticket create --parent`, re-parent, mirror cross-group blocks); record contracts, decisions, pass minor/major classifications, known limits, and final report via `ur ticket add-activity`; file follow-ups via `ur ticket create` + `add-link`; set `pr_summary` on top ticket before `step-complete`.
- **Unattended overrides for driven skills:**
  - Tell implementer subagents NOT to call any `ur-workertools status` command (return summary or `BLOCKED: <reason>` instead).
  - Orchestrator runs `@%UR_SKILL_HOOKS%/implement/subtask-verifications.md` after each batch/Trim (fallback to `CLAUDE.md` commands if absent) and `@%UR_SKILL_HOOKS%/implement/final-verifications.md` before `step-complete`.
  - Override `receiving-code-review`'s "discuss with human" — orchestrator decides against contract and records it.
- Never push, open PRs, or advance lifecycle status.

## Local
- Input can be a design doc, plan, branch prompt, or optional `ur` tickets. If no design exists yet, run `design` with the user *before* starting the autonomous loop.
- No `ur-workertools`, no `@%UR_SKILL_HOOKS%`: verify directly with the repo's documented build/test/lint commands.
- Record contracts and fix batches under `tmp/`; keep a concise decision & known-limits log and present the full final report in chat when finished (units completed, review tiers/passes, decisions, follow-ups, known limits).
- On a Hard stop, finish all unblocked units first, then present the single concrete question + recommended answer in chat.

## Guardrails
- Never implement or review inline; never paraphrase driven skills into subagent prompts (name the skill + pass only contract/lens/paths/output requirements).
- Never run parallel reviewers (`N > 1`) inside `auto` (false agreement, shared blind spots).
- Never spawn a fresh implementer for fix batches when the batch's implementer can be messaged.
- Never trust subagent test claims without verifying directly; never skip writing fix batches to disk.
- Never treat severity (P0–P3) as contract class, never run lenses outside the threat model, never re-review after a minor pass or after Trim, never exceed 3 review passes.
