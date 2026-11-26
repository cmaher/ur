# design-doc-refinements

- **Targets:** worker, local
- **Trigger:** Comparing a design doc, spike, TDD, or RFC against actual repository code and earlier designs, and turning the comparison into a shareable plain-text review page ("refinements") for the author (especially a junior developer).

## Core
- **Research phase:**
  1. Read the full design doc including comments.
  2. Search for earlier designs/RFCs/notes for the same system; read the oldest full design first (its Q&A/limitations often reveal recurring traps).
  3. Verify claims against code via a background search agent: `git fetch` first, read **only `origin/<default>`** (and any unmerged branches the doc cites) — never a stale local checkout. Give the agent a numbered claim list and require `file:line` evidence; resolve full repo paths (`git ls-tree`) before citing.
- **Refinements page structure:**
  1. Header: link to doc, checked branch, and date.
  2. Numbered Refinements: eyebrow (`Refinement N`) + finding-statement `<h2>`, lead-in + grey blockquote of doc claim, plain prose/table findings, red gap callout (only when a gap exists; states what the doc should change, never lectures), and a copyable **Prompt** box naming exact code/search locations. Common checks: end-to-end flow diagram with gap stops, silent missing-entry lookups, unmerged flags/deps, empty allowlist/default semantics, vague scope words, real setting locations, lessons from earlier designs.
  3. Conditional final refinement for Open Questions / TBD lists (skip if doc has none): table of status (answered / partly answered / open) and where answers belong, counting answers found earlier; one gap callout for stale-open or missing questions.
  4. "What the doc got right" section.
  5. Edits checklist persisted in `localStorage` wrapped in `try/catch`.
  6. Sources.
  7. Next iteration: two prompts — one to apply refinements after re-verifying against code, one to generate a fresh refinements page invoking this skill by name.
- **Style & visual discipline:**
  - Calm, neutral tone (never lecturing/classroom framing; no "Lesson"/"Point"/"L1" prefixes).
  - Plain prose and plain tables; highlight **only gaps** in red (no colored pills on every row, no color key, no label tags like "Assumption"/"Code says").
  - Sidebar shows number + 2–4 word title (`·` for non-refinement sections); full-column text (no `max-width: 68ch` caps); every number has a unit ("~64 points").
  - Only use link formats supplied by tools/hooks — otherwise cite `path:line` as code.

## Worker
- Include optional project hook `@%UR_SKILL_HOOKS%/design-doc-refinements/tools-and-links.md` for doc search tools and URL link formats (skip silently if absent; never invent tools or URL formats).

## Local
- No `@%UR_SKILL_HOOKS%`; use whatever doc-search tools/URLs are available in the session, or cite `path:line` directly without inventing link formats.

## Guardrails
- Never describe code paths from memory — require `file:line` evidence from `origin/<default>`.
- Never state counts ("3 of 8 answered") without recounting against the doc.
- Never invent channels, people, tools, or URL formats.
- Never republish to a new file path (keep the same path so the page URL stays stable).
<!-- REVIEW: You noted you've never used this one — review whether to keep both targets, make it local-only, or drop it before Phase 2. -->
