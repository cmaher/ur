---
description: Load this project's brain — user-managed context mounted at /brain. Use when you need durable project knowledge (architecture, decisions, domain facts) or in-progress working context that isn't in the repo.
---

# Brain

The **brain** is a per-project, user-managed context directory mounted read-write at `/brain`. It holds knowledge that lives outside the repo: durable facts (architecture, decisions, domain knowledge) and transient working context (investigations, scratch notes). Its content is owned by the user — this skill only reads it.

## Instructions

1. **Read `/brain/CLAUDE.md`** — the top-level orientation file.
   - If it does **not** exist, the brain is empty. Tell the user the brain is empty and suggest running `/brain:init` to scaffold it, then stop.
2. **Follow the table-of-contents links** in `/brain/CLAUDE.md`. It points to two sub-indexes:
   - `/brain/evergreen/CLAUDE.md` — durable knowledge (architecture, decisions, domain facts).
   - `/brain/working/CLAUDE.md` — in-progress / transient context (investigations, scratch).
   Each index lists one-line pointers to sibling files (the `MEMORY.md` index pattern).
3. **Load the entries relevant to your current task** by reading the files those pointers reference. Do not read the whole brain indiscriminately — follow the pointers that matter for what you're doing.

The brain is user-managed. Do not restructure or delete brain content here — use `/brain:init` only to scaffold missing layout.
