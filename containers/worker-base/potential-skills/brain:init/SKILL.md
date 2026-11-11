---
name: brain:init
description: Scaffold or finish setting up the per-project brain — a top-level orientation CLAUDE.md, reference/ and working/ table-of-contents stubs, a git repository, and the dream config. Non-destructive: only creates what is missing, never overwrites existing brain content. Use to initialize an empty brain, or one that isn't under git or has no dream config.
---

# Brain Init

Scaffold the **brain** (the per-project, user-managed context directory). This is **non-destructive and additive only**: create each directory and file **only where it is missing**, and **never overwrite, edit, or delete** any existing brain file.

## Brain root

`<brain>` is `$UR_BRAIN` when set (ur sets it to `/brain` in workers that have a brain). Otherwise it is the `brain` field of this project in `ur project list --output json`: the project is this skill's argument, or the one whose `repo` matches this directory's `origin` remote; if none or several match, ask. Run every git command from inside `<brain>` (`cd <brain>` first) — never `git -C`, because in a worker only the working directory is mapped to the host.

Target layout:

```
<brain>/
  .git/                <- the brain is its own git repository
  .gitignore
  CLAUDE.md            <- top-level orientation; links to the two ToCs below
  reference/CLAUDE.md  <- ToC for durable knowledge (architecture, decisions, domain facts)
  working/CLAUDE.md    <- ToC for in-progress/transient context (investigations, scratch)
  DREAM.md             <- configures /brain:dream, filled in by /brain:dream-init
```

## Instructions

**Existing brain:** if `<brain>/CLAUDE.md` already exists, skip steps 3 and 4 — the brain already has its own index conventions (it may deliberately leave a folder unindexed), and stubs would contradict them.

For each item below, first check whether it already exists. If it exists, **leave it untouched** and record it as "already present". If it is missing, create it and record it as "created".

1. **Directories**: ensure `<brain>/`, `<brain>/reference/` and `<brain>/working/` exist (create only if missing).

2. **`<brain>/CLAUDE.md`** (only if missing) — top-level orientation. Write a short intro explaining that this is the project's brain (user-managed context, mounted at `/brain` in workers), followed by one-line pointers to the two sub-indexes:
   - `reference/CLAUDE.md` — durable, long-lived knowledge (architecture, decisions, domain facts).
   - `working/CLAUDE.md` — transient, in-progress context (investigations, scratch notes).

3. **`<brain>/reference/CLAUDE.md`** (only if missing) — a table-of-contents stub. Explain that `reference/` holds **durable, long-lived** knowledge, and that this file is an index following the one-line-pointer convention (one `- [Title](file.md) — one-line hook` per sibling file, added as content grows). Leave the index empty for now.

4. **`<brain>/working/CLAUDE.md`** (only if missing) — a table-of-contents stub. Explain that `working/` holds **transient, in-progress** context, and that this file is an index following the same one-line-pointer convention. Leave the index empty for now.

5. **`<brain>/DREAM.md`** (only if missing) — a stub saying this file configures `/brain:dream` and is filled in by `/brain:dream-init`. Write nothing project-specific into it.

6. **Git** — the brain is its own repository:
   - If `<brain>/.git` exists, leave it alone and record "already present". Never re-initialize, change branches, or rewrite history.
   - Otherwise, from inside `<brain>`: `git init`, and create `.gitignore` (only if missing) ignoring OS and editor clutter (`.DS_Store`, `*.swp`). Do not ignore `.dream/`.
   - Commit everything present: `git add -A`, then `git commit -m "brain: init"`. Use the user's normal git config; never pass `--no-gpg-sign` or override the author. If a git command fails, stop and show the error.
   - Never add a remote. If the brain already had one (an existing repository), push the commit.

## After scaffolding

Report exactly what was **created** versus what was **already present**, per path, and the commit made.

Then, if `<brain>/DREAM.md` has no ```` ```toml ```` config block (it is the stub), run `/brain:dream-init` to fill it in. If that skill isn't installed here, say that dream config is still to do and that `/brain:dream-init` is available on the host and in manual workers. If `DREAM.md` is already configured, skip this and say so.
