---
description: Load this project's brain — user-managed context outside the repo (mounted at /brain in workers). Use when you need durable project knowledge (architecture, decisions, domain facts) or in-progress working context that isn't in the repo.
---

# Brain

The **brain** is a per-project, user-managed context directory. It holds knowledge that lives outside the repo: durable facts (architecture, decisions, domain knowledge) and transient working context (investigations, scratch notes). Its content is owned by the user — this skill only reads it.

## Brain root

`<brain>` is `$UR_BRAIN` when set (ur sets it to `/brain` in workers that have a brain). Otherwise it is the `brain` field of this project in `ur project list --output json`: the project is this skill's argument, or the one whose `repo` matches this directory's `origin` remote; if none or several match, ask.

## Instructions

1. **Read `<brain>/CLAUDE.md`** — the top-level orientation file.
   - If it does **not** exist, the brain is empty. Tell the user the brain is empty and suggest running `/brain:init` to scaffold it, then stop.
2. **Follow the table-of-contents links** in `<brain>/CLAUDE.md`. It typically points to sub-indexes such as:
   - `<brain>/reference/CLAUDE.md` — durable knowledge (architecture, decisions, domain facts).
   - `<brain>/working/CLAUDE.md` — in-progress / transient context (investigations, scratch).
   Each index lists one-line pointers to sibling files (the `MEMORY.md` index pattern).
3. **Load the entries relevant to your current task** by reading the files those pointers reference. Do not read the whole brain indiscriminately — follow the pointers that matter for what you're doing.

## Writing to the brain

When you start a new stream in `<brain>/working/`, give its entry file (`README.md`, or the single file) frontmatter so later consolidation can tell when it is finished without searching for it:

```yaml
---
status: active          # active | done | archived
refs:                   # tickets, PRs, branches this work lives in; kinds are listed in DREAM.md
  - ticket: EXAMPLE-123
  - pr: example-org/example-repo#45
last-verified: YYYY-MM-DD
---
```

Add refs as they appear. If `<brain>/DREAM.md` lists ref kinds, use those names.

The brain is user-managed. Do not restructure or delete brain content here. `/brain:init` scaffolds missing layout; `/brain:dream` is the only skill that proposes restructuring, and it applies changes only when asked.
