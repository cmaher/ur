---
name: design
description: Use when starting any creative work — new features, components, behavior changes, or architectural design — or when asked how to build something. Do not use for bug fixes, debugging, pure refactors, code questions, or when told to skip design.
---

# Design

Turn ideas into clear, approved designs through collaborative dialogue before any code is written.

<HARD-GATE>
Do NOT write or scaffold source code before the design is presented and approved. Exception: wrilting or saving a documentation file the user explicitly requested.
</HARD-GATE>

No task is "too simple" to design — small changes get a brief design (a few sentences) and explicit approval before implementation.

## Process

Follow these steps in order:

1. **Explore context (targeted):** read relevant `CLAUDE.md`/`AGENTS.md` files, codeflows, and directly related code. Do not speculatively dump unrelated files or ticket lists.
2. **Check scope:** if the ask spans multiple independent subsystems, flag it immediately and help decompose into sub-projects, then design one at a time.
3. **Clarify — one question at a time:** ask **one** focused question per message (prefer multiple choice when helpful) targeting purpose, constraints, and success criteria.
4. **Propose 2–3 approaches:** present concrete alternatives with trade-offs, leading with your recommended option and why.
5. **Present the design incrementally:** scale each section to complexity (a few sentences up to ~300 words) covering architecture, components/interfaces, data flow, error handling, and testing. Confirm approval after each section before moving on.

## Design Quality Bar

- **Isolation and clarity:** break work into small, single-purpose units with clear interfaces understandable without reading internals and changeable without breaking consumers.
- **Fit existing codebase:** follow established patterns; include targeted cleanup only where existing code directly impedes the feature (no unrelated refactors).
- **Ruthless YAGNI:** drop unrequested features and speculative abstractions.
- **Resolve judgment calls now:** settle architectural and behavioral choices in the design rather than deferring them to implementation.

## Output (After Design Approval)

Produce the artifact that fits the user's workflow:
- A concise approved design summary in chat, **or**
- A design/plan doc under `docs/plans/` if requested, **or**
- `ur` parent + child tickets (if working in a `ur`-tracked project and asked to file tickets).

Stop cleanly once the design artifact or tickets are approved — do not jump into coding uninvited.

## Guardrails

- Never skip design because a task "looks simple".
- Never ask multiple questions in one message during discovery.
- Never jump straight to one solution without proposing 2–3 alternatives and trade-offs.
- Never write source code or start implementing before the design is approved.
<!-- compiled-from: skills/specs/design.md@c1fb7c5bae70 profile:local@daa4419e0589 -->
