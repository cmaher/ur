# design

- **Targets:** worker, local
- **Trigger:** Creative work — new features, components, behavior changes, architecture/system design, or "how should we build X". Do NOT trigger for bug fixes, debugging, pure refactors, code questions, or when user says to skip design.

## Core
- **No implementation during design:** hard gate against writing/scaffolding source code or transitioning to implementation. Exception: writing a documentation file the user explicitly asked to save/create.
- **No project is "too simple":** even small changes get a brief design + user approval before implementation.
- **Ordered collaborative process:**
  1. **Explore context:** read relevant `CLAUDE.md`/`AGENTS.md`, codeflows, and targeted code (no speculative bulk dumps).
  2. **Scope check:** if the ask spans multiple independent subsystems, decompose into sub-projects first and design one at a time.
  3. **Clarify one question at a time:** prefer multiple choice when helpful; focus on purpose, constraints, and success criteria. Never overwhelm with a wall of questions.
  4. **Propose 2–3 approaches:** include trade-offs and lead with a clear recommendation.
  5. **Present design incrementally:** scale each section to complexity (a few sentences up to ~300 words), covering architecture, components/boundaries, data flow, error handling, and testing; get approval after each section.
- **Design quality:**
  - Isolate small, single-purpose units with clear interfaces (understandable without reading internals; internals changeable without breaking consumers).
  - Follow existing repo patterns; include targeted cleanup only where existing code directly impedes the feature (no unrelated refactors).
  - Apply YAGNI ruthlessly.
  - Resolve all judgment calls in the design — never defer architectural choices to the implementer.

## Worker
- **Terminal output is `ur` tickets only** (never suggest coding or invoke implementation/planning skills):
  1. Create a NEW parent ticket with the full approved design document as body: `ur ticket create "Title" --body "..." --output json`.
  2. Register parent ticket with workerd: `ur-workertools workflow set-ticket <parent-id>`.
  3. Create discrete child tickets (`--parent <parent-id>`, no `--wip` so they stay `open`/dispatchable), ordered with `add-block` (hard deps) and `add-link` (related).
  4. If invoked on a `design`-type ticket: inspect it with `ur ticket show` + `ur ticket list --tree`, link it to the new parent (`add-link`), and close the design ticket (`--status closed`) once children are created.
- **Calibrate child tickets for smaller/faster implementers (e.g., Sonnet):**
  - Explicit file paths and patterns (never "the relevant module").
  - Small coherent scope: 1–3 files, one concern per ticket (split cross-layer work).
  - Required child ticket body template: `Description`, `Context`, `Files to read first`, `Files to change`, `Out of scope`, `Acceptance Criteria`.
- **Note:** Design workers are interactive with the user (do not use `AskUserQuestion` tool — ask in normal chat; do not enter plan mode).

## Local
- Same collaborative exploration → 1-question-at-a-time → 2–3 approaches → incremental design approval flow.
- Output matches what the user wants: chat design summary, a design doc under `docs/plans/`, or `ur` tickets (if working in a `ur`-tracked project and asked to file tickets).
- Never jump into coding before design approval; stop cleanly once the design artifact/tickets are approved.

## Guardrails
- Never skip design because a task "looks simple".
- Never ask multiple questions in one message during discovery.
- Never jump straight to one solution without presenting alternatives and trade-offs.
- Never leak implementation code or transition straight into coding from the design phase.
