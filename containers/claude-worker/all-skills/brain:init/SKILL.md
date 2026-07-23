---
name: brain:init
description: Scaffold the per-project brain layout under /brain — a top-level orientation CLAUDE.md plus evergreen/ and working/ table-of-contents stubs. Non-destructive: only creates files that are missing, never overwrites existing brain content. Use to initialize an empty brain.
---

# Brain Init

Scaffold the initial **brain** layout under `/brain` (the per-project, user-managed context directory mounted read-write). This is **non-destructive and additive only**: create each directory and file **only where it is missing**, and **never overwrite, edit, or delete** any existing brain file.

Target layout:

```
/brain/
  CLAUDE.md            <- top-level orientation; links to the two ToCs below
  evergreen/CLAUDE.md  <- ToC for durable knowledge (architecture, decisions, domain facts)
  working/CLAUDE.md    <- ToC for in-progress/transient context (investigations, scratch)
```

## Instructions

For each item below, first check whether it already exists. If it exists, **leave it untouched** and record it as "already present". If it is missing, create it and record it as "created".

1. **Directories**: ensure `/brain/evergreen/` and `/brain/working/` exist (create only if missing).

2. **`/brain/CLAUDE.md`** (only if missing) — top-level orientation. Write a short intro explaining that this is the project's brain (user-managed context, mounted at `/brain`), followed by one-line pointers to the two sub-indexes:
   - `evergreen/CLAUDE.md` — durable, long-lived knowledge (architecture, decisions, domain facts).
   - `working/CLAUDE.md` — transient, in-progress context (investigations, scratch notes).

3. **`/brain/evergreen/CLAUDE.md`** (only if missing) — a table-of-contents stub. Explain that `evergreen/` holds **durable, long-lived** knowledge, and that this file is an index following the one-line-pointer convention (one `- [Title](file.md) — one-line hook` per sibling file, added as content grows). Leave the index empty for now.

4. **`/brain/working/CLAUDE.md`** (only if missing) — a table-of-contents stub. Explain that `working/` holds **transient, in-progress** context, and that this file is an index following the same one-line-pointer convention. Leave the index empty for now.

## After scaffolding

Report exactly what was **created** versus what was **already present**, per path. Do not touch any file marked already present.
