# address-feedback

- **Targets:** worker
- **Trigger:** Turning PR review comments on an active `ur` epic/ticket into follow-up child tickets and concise in-thread PR replies (invoked with `<epic_id> <pr_number>`).

## Core
- Parse two required positional arguments: `<epic_id>` and `<pr_number>`.
- **Fetch context:**
  - `ur ticket show <epic_id> --output json` (capture `project`, `title`, `priority`).
  - `ur ticket list --parent <epic_id> --output json` to avoid creating duplicate child tickets for already-tracked feedback.
- **Fetch all PR comments:**
  - Resolve `{owner}/{repo}` from `gh pr view <pr_number> --json url` or `gh api`.
  - Fetch both inline review comments (`pulls/<pr>/comments --paginate`) and top-level conversation comments (`issues/<pr>/comments --paginate`).
  - Keep bot comments (CI/linter feedback is actionable); skip auto-replies starting with `🤖`.
- **Triage into two buckets:**
  - **Ticket-worthy:** actionable code changes (bugs, missing error handling, refactors, performance, substantive suggestions). Group related comments on the same concern into a single child ticket.
  - **Reply-only:** nits, questions, discussion, praise/acknowledgments.
  - If neither bucket has items, skip straight to completion.
- **Create child tickets & link comments:**
  - Create each ticket with `ur ticket create "<concise actionable title>" --parent <epic_id> --priority <parent_priority> --body "..." --output json`, embedding reviewer, PR#, file:line (omit for top-level comments), comment URL, and full comment text (all merged comments if grouped).
  - Link every source comment in the group via `ur-workertools repo comments link --pr <pr_number> --comment <comment_id> --ticket <ticket_id>` (one call per comment so the server posts tracking replies).
- **Reply to non-ticket comments:**
  - Post via `ur-workertools repo comments reply --pr <pr_number> <comment_id> "<message>"`.
  - Address nits specifically, answer questions directly (admit uncertainty if unknown), engage substantively on discussions, and skip pure praise/acknowledgments.
  - Tone: terse, technical, factual; zero performative agreement ("Great point!").

## Worker
- End via `_profiles/worker.md` stop contract: `ur-workertools status step-complete` when done, or `ur-workertools status request-human "<reason>"` if blocked.

## Guardrails
- Never skip bot comments; always skip `🤖` poller auto-replies.
- Never duplicate feedback already tracked in an existing child ticket.
- Never forget to link every source comment in a merged group via `repo comments link`.
- Never use performative flattery in PR replies.
<!-- REVIEW: Currently hardcoded to `gh api` (GitHub). Let me know if you want Gitea/`tea` support added here or keep GitHub-only for now. -->
