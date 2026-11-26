---
name: design-doc-refinements
description: Use when comparing a design doc, spike, TDD, or RFC against repository code and earlier designs to produce a shareable refinements review page for its author
---

# Design Doc Refinements

Check a design doc's claims against repository code and earlier designs, then publish findings as a calm, plain-text review page of numbered **refinements**. Each refinement states one finding, shows evidence, names the gap, and ends with a reusable prompt.

## Optional Hook Files

The `@%UR_SKILL_HOOKS%/...` reference below is an optional project hook supplying doc-search tools and URL link formats. Read and follow it if the file exists. If absent, skip it silently and continue with code and the doc alone — absence is never a blocker, never a reason to call `request-human`, and never a reason to create missing hooks, invent tools/URLs/link formats, or invent replacement checks.

@%UR_SKILL_HOOKS%/design-doc-refinements/tools-and-links.md

## Research Phase

1. **Read the full doc**, including all comments when available.
2. **Read earlier designs:** if doc search tools are available, find earlier designs, RFCs, or notes for the same system and read the **oldest full design first** (its Q&A and limitations sections often surface recurring traps).
3. **Verify code claims in a background search agent:**
   - Run `git fetch` first, then read **only `origin/<default>`** (`git show`, `git grep` against `origin/<default>`) plus any unmerged branches the doc cites — never a stale local checkout.
   - Give the agent a numbered list of claims to verify and require `file:line` evidence for each.
   - Resolve full repository paths (`git ls-tree -r --name-only origin/<default> | grep ...`) before citing anything.

## Page Structure

1. **Header:** link to the design doc, checked branch, and date checked.
2. **Numbered Refinements:** each has a small `Refinement N` eyebrow above an `<h2>` stating the finding, a lead-in line + plain grey blockquote of the doc claim (when quoting the doc), plain prose/table findings, a red gap callout (only when a gap exists), and a copyable **Prompt** box naming exact code/search locations. Common findings to check:
   - End-to-end request flow diagram with red-outlined gap stops
   - Lookups that fail silently on missing entries
   - Feature flags and unmerged dependencies
   - Empty allowlist or default semantics
   - Vague scope words
   - Where a setting actually lives (code vs external system)
   - Lessons from earlier designs
3. **Open Questions refinement (conditional):** include ONLY if the doc has an Open Questions / TBD / unresolved list (skip otherwise). Provide a plain-text status table (`answered`, `partly answered`, `still open`) showing where each answer belongs (counting answers found in earlier refinements), plus one gap callout for answered questions still listed as open and new questions missing from the list.
4. **What the doc got right.**
5. **Edits checklist** persisted in `localStorage` wrapped in `try/catch`.
6. **Sources.**
7. **Next iteration:** two reusable prompts — one that applies the refinements after re-verifying against code and sources, and one that generates a fresh refinements page invoking the `design-doc-refinements` skill by name (noting both work best with the skill installed).

## Style & Visual Discipline

| Do | Don't |
| --- | --- |
| Eyebrow `Refinement N` + finding-statement `<h2>` | Topic/method headings or `Lesson`/`Point`/`L1` prefixes |
| Sidebar rows: number + 2–4 word title (`·` for non-refinement sections) | Bare numbers with no titles |
| Plain prose and plain tables | Label badges like `Assumption`/`Code says` or a color key |
| Highlight **only gaps** in red (callout, diagram stops, `"Nothing"` cells) | Colored pills on every row |
| Lead-in line (`The doc says (...):`) before a plain grey blockquote | Opening a section with a bare epigraph quote |
| Copyable **Prompt** box naming where to look (`Check the code: ...`) | Advice telling the reader how to work (`Before writing X, grep Y`) |
| Gap callouts stating what the doc should change, in a calm neutral tone | Lecturing, classroom framing, or instructing the reader how to work |
| Full-column prose and callouts; units on every number (`~64 points`) | `max-width: 68ch` caps or unitless numbers (`~64`) |
| Link formats supplied by tools/hooks, or `path:line` as code | Invented URL or link formats |

## Guardrails

- Never describe a code path from memory — cite `file:line` evidence verified against `origin/<default>`.
- Never state counts (`3 of 8 answered`) without recounting against the doc.
- Never invent channels, people, tools, or URL formats.
- Never republish to a new file path — reuse the same path so the page URL stays stable.
<!-- compiled-from: skills/specs/design-doc-refinements.md@28af473e9e7f profile:worker@6afed0c52191 -->
