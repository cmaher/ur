---
name: auto
description: Use when asked to keep going on a multi-step build on your own ("keep going", "work autonomously", "auto", "don't stop unless you must"), when working designed tickets unattended, or when work needs implementing and independent review against an explicit scope, with review depth scaled to risk (security boundaries, auth, credentials, data migrations, public API). Drives the implement and code-review skills in a contract-bounded loop that runs to completion without human intervention.
---

# auto: skill-driven work-and-review harness

## Overview

You are the **orchestrator**. You do not implement or review by hand. You drive our skills through subagents, then
decide, verify and commit. Keep your own context small: read reports and diffs, not whole codebases.

**Core principle:** nothing counts until you have verified it yourself, and a unit is done when it meets its **scope
contract**, not when reviewers run out of ideas. Adversarial reviewers can always find another corner. The contract
decides which corners matter.

**Judgement:** review passes cost real wall-clock time. Spend them where a missed defect is expensive. Don't loop on
low-risk work just because a reviewer can always find another corner. The contract's **review depth** (below) sets
how many passes a unit gets.

## Unattended by default: do not stop

This harness runs on **already-designed tickets** with **no human watching**. Your response text is not delivered to
anyone. The whole point is that it finishes without intervention. Treat every stop that isn't a Hard stop as a
failure of the run.

There are exactly two legal ways to end the run:

| Situation | Required action |
| --- | --- |
| Every unit is done and the Subtraction pass is verified | `ur-workertools status step-complete` |
| A Hard stop (below), after all other work is finished | `ur-workertools status request-human "<reason>"` |

- **Run until every unit is done.** Do not end your turn with a status update, a "next I'll...", a question, or a menu
  of options while dispatchable work remains. Waiting on a subagent is not a reason to end the turn; you are
  re-invoked when it finishes.
- **Decide, record, continue.** Every judgment call this skill describes (scope, threat model, drift, review cap,
  buy-vs-build, `BLOCKED:` from an implementer) is yours. Pick the narrowest reading that satisfies the design ticket,
  record the decision on the ticket with `ur ticket add-activity <id> "decision: ..."`, and keep going. The user
  reviews decisions afterwards from the ticket log and your final report.
- **Never ask a question in your response text, never use AskUserQuestion, and never tell a subagent to ask the user.**
- `ur-workertools status pause-nudge` only suppresses nudges while you keep working. It is not a way to stop.

## Skills this harness drives

| Stage | Skill | Who runs it |
| --- | --- | --- |
| Design + ticket breakdown (only before the run, when no ticket exists yet) | `design` | You, with the user |
| Implement a unit / apply fix batches | `implement` | Implementer subagent |
| Review pass (one lens per pass) | `code-review` | Reviewer subagent |
| Triage reviewer findings | `receiving-code-review` | You |

Every subagent prompt names the skill to invoke explicitly ("Invoke the `implement` skill on ticket `<id>`"). Do not
paraphrase a skill's instructions into the prompt; the skill is the source of truth. The prompt adds only what the
skill can't know: the contract, the lens, owned paths, and the output you need back.

**Unattended overrides.** Some driven skills say to stop and ask, or to signal the workflow. Under `auto`, the
orchestrator is "the user" and the only one who signals the workflow, and this skill's rules win:

- `implement` `request-human` / `step-complete` → the implementer subagent must NOT run any `ur-workertools status`
  command. Tell it so in every prompt: "instead of `request-human`, return `BLOCKED: <reason>`; instead of
  `step-complete`, return your summary". Settle `BLOCKED:` per the Scope drift table.
- `implement` "do not run verification commands unless specified" → that limits the implementer, not you. You verify
  (step 6).
- `receiving-code-review` "discuss with your human partner" → you decide against the contract and record it.

Pushing, PRs and lifecycle advancement are the workflow's job after `step-complete`. Never push, open a PR, or
advance lifecycle status yourself.

## Before the first unit

1. **Error recovery.** `ur ticket --output json show <id> --activity-author workflow` on the ticket you were given.
   If the most recent workflow activities report a failure (verification, CI, merge) that isn't addressed yet, the
   first unit is fixing it: write a contract for the fix and run it through the loop.
2. **Record the run base:** `git rev-parse HEAD`, for the end-of-run Subtraction pass.

## The unit and its ticket

A **unit** is a **batch**: one leaf ticket when it is big enough on its own, otherwise a sub-epic of related leaf
tickets (one area, dependency-ordered, ~500-1500 lines of non-test code together). The normal input is a designed
ticket or epic; its design is the source of truth for requirements. If the work has no ticket, the `design` skill
runs with the user **before** the autonomous run starts. Once the loop begins, no more design questions go to the
user. Small asks with an obvious shape can go straight to a ticket via `ur ticket create`.

**This skill's sizing supersedes the `design` skill's.** `design` sizes tickets for individual workers (1-3 files
each). Under `auto`, that granularity is input, not the unit: a per-ticket loop (fresh agent, contract, review) costs
more than it catches on small tickets. When an epic arrives as many small tickets, first group them into sub-epics
(`ur ticket create ... --parent <epic>`, re-parent members, mirror cross-group blocks as sub-epic blocks so the
sub-epics form a DAG), then run the loop per sub-epic. One implementer works every ticket of its batch in order under
one contract; one review pass (per tier) covers the batch's whole diff; the implementer closes each ticket as it
lands.

**Breaking down a design mid-run.** When a design ticket must become work during the run, write the breakdown as
batches, not per-file tickets: name, goal, areas/files, size, prerequisites, key decisions. File one ticket per batch
(the batch's implementer writes its own step list from the contract). Keep plans short (a page per design, not full
ticket bodies). Before filing a large breakdown (> ~3k lines), check its scope against the cheapest reading of the
user's ask: prefer keeping an existing working system, porting only what a stated goal needs, and dropping work whose
value is doubtful (e.g. a site integration the user may lose access to). Record one short scope note on the epic
(`ur ticket add-activity <epic> "scope: about to break down X: ~N lines across K batches; ..."`) and **keep going**.

**One batch in progress at a time.** For an epic, do NOT hand the epic to `implement` (it would work the whole epic
in one context). Take the next batch (a dispatchable sub-epic, or a dispatchable leaf that is a batch on its own) from
`ur ticket --output json dispatchable <epic-id>`, run it through the whole loop below (contract, implement, review,
close), then pick the next. Re-query `dispatchable` after each close, and keep going until it returns nothing.

## The scope contract (write it before any code)

Every unit gets a short contract on disk (`tmp/<unit>-contract.md`), approved by you, recorded on the ticket
(`ur ticket add-activity <id> "contract: ..."`), and given **verbatim** to every implementer and reviewer:

- **Requirement:** the user's ask, in their words.
- **Threat model / failure model:** who and what we defend against. Name what we do NOT defend against.
- **Delegated to the platform:** what the stdlib, framework, database, OS or edge proxy already does, so we don't
  write it (e.g. "CSRF: `http.CrossOriginProtection`", "body limits: edge proxy", "least privilege: GRANTs").
- **Non-goals:** things a reviewer will be tempted to add.
- **Size budget:** expected lines of non-test code. Going past 2x triggers a course correction (see below).
- **Review depth:** `Default` or `Critical`, picked from the threat model (see below).

If the design ticket doesn't settle the threat model or non-goals, make the call yourself and pick the narrow
reading. Record it on the ticket and mention it in your report. Never widen scope on a reviewer's say-so; anything
wider becomes a follow-up ticket (`ur ticket create ... --parent <epic>` or linked with `add-link`), not a stop.

### Review depth

| Tier | Applies to | Review passes |
| --- | --- | --- |
| **Default** | Most units: plumbing, config, refactors, UI, internal tooling | ONE pass. Fix its DEFECTs, verify yourself, stop. Re-review only if a fix changed behavior non-trivially |
| **Critical** | Security boundaries, auth, credentials, sandboxing/permission enforcement, crypto, untrusted-input parsing, data loss or migrations on prod data, public API contracts | New lens per pass, strongest model for reviewers. Review again only after a **major** pass (step 7); a **minor** pass ends review. Hard cap 3 |

Size does not lower risk: a ten-line change to an auth check is Critical. When unsure, pick Default and say so.
State the tier in your report. A Critical unit whose first pass is minor is done after that pass.

## The loop

1. **Scope the unit** and write its contract from the design ticket, including review depth. Decide open questions
   yourself and record them.
2. **Implementer subagent → `implement` skill.** Prompt: "Invoke the `implement` skill on ticket `<id>`" (for a
   batch: "on each of tickets `<id1>`..`<idN>`, in this order, as one batch: do not dispatch sub-agents"), plus the
   contract, the paths it owns, "do not push", and the `ur-workertools` override above. `implement` claims, commits
   on the working branch (a commit per ticket is fine), closes each ticket and returns a short summary. One
   implementer, one batch at a time; reuse the same implementer for the batch's fix batches.
3. **Check size against budget first**; past 2x triggers a course correction before any review (see Scope drift).
   **Reviewer subagent → `code-review` skill, ONE at a time** (never parallel, always `N=1`). Prompt: "Invoke the
   `code-review` skill on `<base>..<head>`", plus the contract and a single named **lens** from the catalog. Add these
   requirements on top of the skill's output format:
   - Read-only on the repo; probes go in a scratch dir.
   - **Run a probe for every claim** and mark each finding "confirmed yes/no".
   - Tag every finding with a contract class:
     - **DEFECT:** breaks the requirement, or the threat model as written.
     - **OUT-OF-CONTRACT:** real, but the threat is outside the model, the platform already handles it, or it's a
       non-goal.
     - **NIT:** cosmetic.
   - Return findings and verdict only; no fixes.
   Use the strongest model for Critical units.
4. **Triage yourself, using `receiving-code-review`.** Re-check each DEFECT against the contract; reviewers
   over-classify, and `code-review`'s P0–P3 is not the contract class. A finding can be resolved by **code**, by
   **documenting a known limit** (in the package doc or design), or by **rejection**. Prefer the cheapest one that
   satisfies the contract. OUT-OF-CONTRACT findings go to the known-limits list, not the batch. Write the batch to
   `tmp/<unit>-batch<N>.md` so it survives context loss and agent death.
5. **Send the batch to the SAME implementer** via SendMessage (it keeps context and the `implement` conventions).
   Require every new test to fail when its fix is reverted, and a commit per batch.
6. **Verify yourself:** run the project's verification hooks (below), then `git status` and **size against budget**.
   Subagent reports are claims, not evidence. Interim "waiting on background work" notices are not reports.
7. **Decide whether to review again, by tier.**
   - **Default:** done after step 6. Run one more pass only if a fix changed behavior in a non-trivial way.
   - **Critical:** classify the pass you just triaged, and record "pass N: minor/major, because ..." on the ticket.
     - **Minor:** every accepted finding is a NIT, OUT-OF-CONTRACT, a test gap (a guarantee the code already meets
       that no test pins, e.g. a surviving mutant over correct behavior), a doc fix, or a DEFECT whose fix is small
       and local (one spot, no new mechanism, no change to how the unit works). Apply the batch, verify, and **stop
       reviewing**.
     - **Major:** some DEFECT needs a non-local fix: new machinery, a changed design or data flow, or a fix spread
       across the threat-model surface. Apply the batch, verify, then run the next `code-review` pass with a NEW lens
       to judge the fix.
     Severity labels don't decide this (a P1 test gap is still minor); the size and reach of the fix do.
     **Hard cap: 3 review passes.** If major DEFECTs are still
     appearing at the cap, do not stop the run: have the implementer fix the last pass's DEFECTs, verify, file any
     remaining ones as follow-up tickets linked to the unit, record "review cap hit" with the open list on the
     ticket, and move on. Call it out at the top of the final report.
8. **Close out the unit.** Check `git status` for strays. Confirm each ticket of the batch is closed and has a
   `pr_summary`; record decisions and the known-limits list on it with `ur ticket add-activity`. Then go straight to
   the next dispatchable unit.

### Verification hooks

Run these in step 6 and after the Subtraction pass. If a hook file is absent, fall back to the build, test and lint
commands the project's CLAUDE.md documents.

@%UR_SKILL_HOOKS%/implement/subtask-verifications.md

Before `step-complete`, also run:

@%UR_SKILL_HOOKS%/implement/final-verifications.md

## End of run: one Subtraction pass

No per-unit Subtraction passes. Run it **once**, after the last unit closes and before the final report.

1. **Reviewer subagent → `code-review`** on `<run-base>..HEAD` with the Subtraction lens and every unit's contract.
   Its only job is to find code to delete: anything the platform already does, guards compensating for one root
   cause, config knobs nobody needs, abstractions with one implementation, duplication between units.
2. **Triage** as in step 4. Send the accepted batch to the most recent implementer via SendMessage. If it's gone,
   start a fresh implementer with the `implement` skill on a cleanup ticket (`ur ticket create` under the epic).
3. **Verify** as in step 6 and commit. Subtraction findings never trigger another review pass.
4. **Finish:** set `pr_summary` on the ticket you were given
   (`ur ticket set-meta <id> pr_summary "<summary of all units>" --output json`), record the final report on it with
   `ur ticket add-activity`, then run `ur-workertools status step-complete`.

## Lens catalog (pick by contract, not all of them)

Each lens is passed to `code-review` as the reviewer's focus. Choose lenses that test the contract's threat model. A
lens whose threats are all non-goals is skipped (e.g. no DoS lens for an internal admin tool whose contract excludes
availability attacks).

| Lens | What the reviewer does |
| --- | --- |
| Contract conformance | Map every guarantee in the contract to code and a test that really exercises it |
| Protocol / crypto / parsing | Edge-case inputs, encodings, size limits, fail-closed paths, within the threat model |
| Integration / misuse | Build a realistic consumer from the docs ONLY; find pit-of-failure APIs and doc lies |
| Mutation testing | Reviewer designs its OWN mutants (implementer's self-chosen mutants are not trusted) and lists survivors with killing tests |
| Real dependency / real browser | Drive the real service or browser end to end; record actual headers and behavior |
| Availability / DoS | Outages, slow peers, floods, connection and memory bounds, recovery. Only if the contract includes it |
| Fresh read | Whole unit top to bottom by someone who saw no prior pass |
| Subtraction | Once per run, over the whole run's diff. See "End of run" |

## Scope drift: automatic course corrections

These signals used to mean "stop and ask". Unattended, they mean **you** correct course, record why, and continue.
Default to the cheapest option that still meets the design ticket.

| Signal | What you do (no stop) |
| --- | --- |
| Non-test code **past 2x the size budget** | Send the implementer a cut batch (no review pass): shrink to the platform-native minimum. Record the overrun and the cut on the ticket |
| A fix would **re-implement something the platform provides** | Use the platform feature, or mark the finding OUT-OF-CONTRACT. Never build the duplicate |
| The unit **needs its own machinery** (preflight, detection logic, config enum, builder type) | Drop it unless the design ticket requires it; record it as a known limit or follow-up ticket |
| The **same surface has DEFECTs in three passes in a row** | Have the implementer redo that surface from the contract (not patch it again), then run one Fresh read pass |
| Implementer returns `BLOCKED:` | Settle it from the design ticket, contract, codebase and docs, then SendMessage the decision back. If it's truly ambiguous, choose the narrowest reading and record it |
| Severity drifting down, guards piling up, or the code no longer matches the ask | Freeze scope: document the rest as known limits or follow-ups and close the unit |
| Buy-vs-build | Prefer an existing dependency or platform feature already in the repo. Add a new third-party dependency only if the design names it; otherwise build the minimum and record it |

## Hard stops (the only ones)

Run `request-human` only when continuing would be unsafe or impossible, not when it's just consequential:

- An action is **irreversible or destructive to data or shared state** and the run hasn't authorized it: dropping or
  migrating production data, deleting other people's work, publishing externally. (Committing locally and running
  tests is never one of these.)
- **Credentials, access, or infrastructure are missing** and can't be obtained in the environment, and no other
  dispatchable work is left to do in the meantime.
- **The design ticket contradicts itself or the codebase** so badly that every reading would build something the
  user didn't ask for. Before stopping, finish every other dispatchable unit.

Before a hard stop, record on the ticket what blocked it and what you tried, and finish all other work that isn't
blocked. Then run `ur-workertools status request-human "<one concrete question, with your recommended answer>"`.

## Reporting

Decisions go on tickets via `ur ticket add-activity` as you make them. The final report goes on the ticket you were
given, before `step-complete` or `request-human`: units completed, decisions made, review caps hit, follow-up tickets
filed, known limits, and any hard stop. If the user redirects mid-run, update tickets, contracts, design docs and
running agents immediately, then continue.

## Red flags

| Thought | Reality |
| --- | --- |
| "I'll just implement/review this bit inline" | Drive it through `implement` / `code-review` in a subagent |
| "I'll summarize the skill in the prompt" | Name the skill; add only contract, lens, paths, required output |
| "The implementer says tests pass" | Run the verification hooks yourself |
| "The implementer can signal step-complete when done" | Only you signal the workflow; subagents return summaries or `BLOCKED:` |
| "Run the full loop to be safe" | Pick the tier from the threat model; Default is one pass |
| "This touches auth/creds/sandbox but it's small" | Size doesn't lower risk; Critical tier |
| "Hand the epic to `implement`" | One batch through the whole loop at a time |
| "One fresh agent and review per small ticket" | Group related leaves into a sub-epic batch; per-ticket loops cost more than they catch |
| "The `design` skill says 1-3 files per ticket" | Under `auto`, this skill's batch sizing wins |
| "Break the design into dozens of detailed tickets" | Batches with a goal, areas and size; the implementer plans the steps |
| "The design says port it all, so port it all" | Check scope against the cheapest reading; record a scope note and keep going |
| "Run reviewers in parallel / `code-review N>1` to save time" | Same blind spots, false agreement. Sequential, `N=1` |
| "Spawn a fresh implementer for fixes" | Reuse the one with context via SendMessage |
| "The reviewer probably verified it" | Require probes and confirmed yes/no |
| "It's P1, so it's a defect" | P-level isn't the contract class; most late findings are OUT-OF-CONTRACT |
| "It's real, so it needs code" | A documented known limit is a valid resolution |
| "One more pass will get it clean" | A minor pass ends review. Hard cap is 3. Past that, file follow-ups and move on |
| "Critical means passes until two come back clean" | Only a major pass earns another; fix test gaps and nits, then stop |
| "Keep hardening this 900-line custom piece" | You're past budget; subtract down to the platform-native minimum |
| "Add one more guard" | Look for the single root cause the guards are compensating for |
| "I should check with the user before continuing" | Unless it's a Hard stop: decide, record on the ticket, continue |
| "I'll give a progress update and end my turn" | Ending the turn without `step-complete` or `request-human` stalls the workflow |
| "Implementer is BLOCKED, so I'll request-human" | Settle it from the design and codebase; record the decision |
| "The `design` skill says to explore with the user first" | The tickets are already designed. Don't re-run `design` mid-run |
| "A driven skill says stop and ask" | See Unattended overrides: you answer for the user |
| "Hit the review cap / budget, so I'll stop and report" | Course-correct per the table, file follow-ups, move to the next unit |
| "Run a Subtraction pass after this ticket" | Once per run, at the end, over the whole diff |
| "Let's run every lens to be safe" | Lenses outside the threat model only manufacture scope |
| "I'll remember the batch" | Write it to disk |
| "I'll push / open the PR when done" | The workflow does that after `step-complete` |

$ARGUMENTS
