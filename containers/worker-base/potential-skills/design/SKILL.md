---
name: design
description: Use when starting any creative work — new features, components, behavior changes, or architectural design — or when asked how to build something. Do not use for bug fixes, debugging, pure refactors, code questions, or when told to skip design.
---

# Design

Turn ideas into clear, approved designs and actionable `ur` implementation tickets through collaborative dialogue.

<HARD-GATE>
Do NOT write or scaffold source code, suggest coding next steps, or invoke implementation or planning skills. Your terminal output is a parent ticket with the full design and discrete child tickets. Exception: if the user explicitly asks to write or save a documentation file, you may write that file.
</HARD-GATE>

No task is "too simple" to design — small changes just get a short design (a few sentences) and explicit user approval.

Ask all questions directly in normal chat. Do NOT use an `AskUserQuestion` tool and do NOT enter plan mode.

## Process

Follow these steps in order:

1. **Explore context (targeted):**
   - If invoked on a `design`-type ticket, read it (`ur ticket show <id> --output json`) and its hierarchy (`ur ticket list --tree <id> --output json`).
   - Read relevant `CLAUDE.md`/`AGENTS.md` files, codeflows, and directly related code. Do not bulk-fetch unrelated ticket lists or dump the repo.
2. **Check scope:** if the ask spans multiple independent subsystems, flag it immediately and help decompose into sub-projects (one parent ticket per sub-project), then design the first one.
3. **Clarify — one question at a time:** ask **one** focused question per message (prefer multiple choice when helpful) targeting purpose, constraints, and success criteria.
4. **Propose 2–3 approaches:** present concrete alternatives with trade-offs, leading with your recommended option and why.
5. **Present the design incrementally:** break into sections scaled to complexity (a few sentences up to ~300 words) covering architecture, components/interfaces, data flow, error handling, and testing. Get approval after each section before continuing.

## Design Quality Bar

- **Isolation and clarity:** small, single-purpose units with clear boundaries understandable without reading internals and changeable without breaking consumers.
- **Fit existing codebase:** follow established patterns; include targeted cleanup only where existing code directly impedes the feature (no unrelated refactors).
- **Ruthless YAGNI:** cut anything unrequested or speculative.
- **Decide everything here:** resolve every judgment call in the design — never defer architectural choices to implementers.

## Creating Tickets (After Design Approval)

### 1. Parent Ticket
- Create a NEW parent ticket holding the full approved design document (architecture, components, data flow, error handling, testing strategy, decisions):
  `ur ticket create "Title" --body "..." --output json`
- Register it with workerd:
  `ur-workertools workflow set-ticket <parent-id>`
- If invoked on a `design` ticket, link it:
  `ur ticket add-link <design-ticket-id> <parent-id> --output json`

### 2. Child Tickets (Calibrated for Smaller/Faster Implementers)
Implementation tickets are run by smaller-context workers (e.g., Sonnet) — be explicit and tightly scoped:
- **Small scope:** 1–3 files on one concern per ticket; split cross-layer work (proto/server/client/migration).
- **Explicit references:** name exact files, symbols, and patterns — never "the relevant module".
- **No `--wip` flag:** children must default to `open` so they are immediately dispatchable.
- Create each via `ur ticket create "Task title" --parent <parent-id> --body "..." --output json` using this exact template:

```markdown
## Description
What to build and why.

## Context
How this component interacts with neighbors — inputs, outputs, shared interfaces.

## Files to read first
- path/to/existing_similar.rs — pattern to follow
- crates/foo/CLAUDE.md — crate conventions

## Files to change
- path/to/file.rs (create/modify)

## Out of scope
- Explicit list of things NOT to touch

## Acceptance Criteria
- [ ] Concrete, verifiable conditions for done
```

- Wire hard dependencies with `ur ticket add-block <id> <dep-id> --output json` and non-blocking relations with `ur ticket add-link <id> <other-id> --output json`.

### 3. Close Design Ticket (Only If Invoked on One)
- `ur ticket update <design-ticket-id> --status closed --output json`

Stop once tickets are created. Do not suggest coding or invoke another skill.

## Guardrails

- Never skip design because a change "looks simple".
- Never ask multiple questions in one message during discovery.
- Never jump straight to one design without presenting 2–3 alternatives and trade-offs.
- Never write source code or transition into implementation.
<!-- compiled-from: skills/specs/design.md@c1fb7c5bae70 profile:worker@6afed0c52191 -->
