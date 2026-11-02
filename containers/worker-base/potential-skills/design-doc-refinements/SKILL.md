---
name: design-doc-refinements
description: Use when asked to compare a design doc, spike, TDD or RFC against the code and earlier designs, or to turn that comparison into a shareable review page ("refinements") for the doc's author, especially a junior developer.
---

# Design Doc Refinements

## Overview

Check a design doc's claims against the code and any earlier designs, then publish the findings as a calm, plain-text review page made of numbered **refinements**. Each refinement states one finding, shows the evidence, names the gap, and ends with a prompt the author can reuse.

## Tools and links

The `@%UR_SKILL_HOOKS%/...` reference below is an optional project hook. Read and follow it if the file exists. If it is absent, skip it and continue with only the code and the doc. Do not invent tools, URLs or link formats.

@%UR_SKILL_HOOKS%/design-doc-refinements/tools-and-links.md

## Research

1. Read the doc in full, including its comments when they're available.
2. If Tools and links gives a way to search other documents, use it to find earlier designs for the same system and any related work, notes or discussion. Read the oldest full design first. Its Q&A or limitations section usually lists what wasn't supported, and those issues tend to come back.
3. Check the code in a background search agent. Fetch first, then read the default remote branch only (`git show`, `git grep` against `origin/<default>`), never a stale local checkout. Give the agent a numbered list of the doc's claims to verify, and ask for `file:line` evidence for each. Also check any unmerged branches the doc or its references mention.
4. Resolve full repo paths (`git ls-tree -r --name-only origin/<default> | grep …`) before citing anything.

## Page rules

| Do | Don't |
|---|---|
| Each section has a small "Refinement N" eyebrow above an `<h2>` that states the finding ("Checkout depends on an unmerged feature flag") | Headings that name a topic or method ("Following one request through the code"); "Lesson", "Point" or "L1" prefixes |
| Sidebar rows show the bare number and a 2–4 word title. Non-refinement sections get a `·` | A sidebar of numbers with no titles |
| Plain prose and plain tables | Labels such as "Assumption", "Code says" or "Technique"; a colour key |
| Highlight **only gaps**: a red callout, red-outlined stops in diagrams, red "Nothing" text in tables | Coloured pills on every row |
| Introduce each quote with a lead-in line ("The doc says (Preferred Solution):"), then a plain grey blockquote | Opening a section with a bare quote, which reads like a book epigraph |
| End each refinement with a copyable **Prompt** box that names where to look: "Check the code: …", or the search tool from Tools and links, or both chained ("… Then search …") | Advice to the reader ("Before you write X, grep Y") |
| Gap callouts say what the doc should change | Gap callouts that tell the reader how to work |
| Use a neutral tone; the reader may be junior | Lecturing or classroom framing |
| Let text fill the column | `max-width: 68ch` caps on prose, callouts or quotes |
| Link files, commits, PRs, branches and other references in the formats Tools and links gives. With no format, cite `path:line` as code | Link formats you invented |
| Give every number a unit ("~64 points") | "~64" |

## Page structure

1. Header: a link to the doc, and the branch and date the code was checked at.
2. Refinements. Each has a lead-in and quote (when the doc makes the claim), plain findings, a gap callout when there is a gap, and a prompt. Common findings:
   - following one request end to end, shown as a flow diagram with gap stops
   - lookups that fail silently when an entry is missing
   - flags and unmerged dependencies
   - what an empty allowlist or default means
   - vague scope words
   - where a setting actually lives (code, or an external system)
   - what earlier designs tell us
3. Only if the doc has an Open Questions, TBD or unresolved list (AI-drafted docs often do): a final refinement that works the answers back into the doc. Skip it otherwise.
   - Show a table of each question, its status (answered, partly answered or still open) and where the answer belongs in the doc.
   - Count answers found in the earlier refinements too.
   - Keep status as plain text. Use one gap callout for answered questions still listed as open and for new questions missing from the list.
4. What the doc got right.
5. An edits checklist, saved in localStorage with try/catch.
6. Sources.
7. Next iteration, with two prompts:
   - One applies the refinements to the doc after re-checking them against the code and any other sources used.
   - One generates a new refinements page for any doc. Mention that both work best with this skill installed, and have the second prompt invoke it by name.

## Common mistakes

- Stating counts ("three of eight answered") without recounting against the doc.
- Describing a code path from memory instead of from the agent's `file:line` evidence.
- Naming a channel, person or URL that no tool returned.
- Republishing to a new path. Keep the same file path so the page URL stays the same.
