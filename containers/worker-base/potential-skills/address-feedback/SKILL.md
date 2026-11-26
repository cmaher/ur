---
name: address-feedback
description: Use when turning PR review comments on an active ur epic or ticket into follow-up child tickets and in-thread PR replies
---

# Address PR Feedback

Parse `$ARGUMENTS` for two required positional arguments: `<epic_id>` and `<pr_number>`.

## Unattended Execution — Exactly Two Ways To Stop

You are running unattended; response text is not delivered to anyone. End your turn ONLY with:
- Done: `ur-workertools status step-complete`
- Blocked: `ur-workertools status request-human "<reason>"`

Never pause, wait, ask questions in response text, or stop without one of those two commands. `ur-workertools status pause-nudge` only mutes nudges while you keep working and never ends your turn. Never push, open PRs, or advance ticket lifecycle status — the workflow handles that after `step-complete`.

## 1. Fetch Ticket Context

1. Run `ur ticket show <epic_id> --output json` and record its `project`, `title`, and `priority`.
2. Run `ur ticket list --parent <epic_id> --output json` to see existing child tickets and avoid duplicating already-tracked feedback.

## 2. Fetch All PR Comments

Resolve `{owner}/{repo}` via `gh pr view <pr_number> --json url` or `gh api repos/{owner}/{repo}/pulls/<pr_number> --jq '.head.repo.full_name'`.

Fetch both inline review comments and top-level conversation comments:
```bash
gh api repos/{owner}/{repo}/pulls/<pr_number>/comments --paginate
gh api repos/{owner}/{repo}/issues/<pr_number>/comments --paginate
```

Extract `body`, `path`, `line`, `html_url`, `id`, and `user.login`.
- **Keep bot comments** (CI/linter feedback is actionable).
- **Skip comments starting with `🤖`** (auto-replies posted by the poller).

## 3. Triage Comments

Classify each comment into one of two buckets (if both buckets are empty, skip straight to Step 6):
- **Ticket-worthy:** actionable code changes (bugs, missing error handling, refactors, performance, substantive suggestions). Group related comments on the same concern into a single ticket.
- **Reply-only:** nits, questions, discussion, praise/acknowledgments.

## 4. Create Child Tickets & Link Comments

For each ticket-worthy group, create one child ticket inheriting `<parent_priority>`:

```bash
ur ticket create "<concise actionable title>" \
  --parent <epic_id> \
  --priority <parent_priority> \
  --body "PR Feedback from PR #<pr_number>

Reviewer: @<user>
PR: #<pr_number>
File: <path>:<line>
Comment: <html_url>

---

<full comment body>" \
  --output json
```

- For top-level PR comments (from `issues/.../comments`), omit `File: <path>:<line>`.
- When grouping multiple comments into one ticket, include every comment's context block and full body.
- Link **every** source comment in the group (one call per comment so the server posts tracking replies):
  `ur-workertools repo comments link --pr <pr_number> --comment <comment_id> --ticket <ticket_id>`

## 5. Reply to Non-Ticket Comments

Post in-thread replies via:
`ur-workertools repo comments reply --pr <pr_number> <comment_id> "<message>"`

- **Nits:** address specifically (fix or explain why not).
- **Questions:** answer directly; say so if you don't know.
- **Discussion:** engage substantively; push back with technical reasoning when warranted.
- **Praise / acknowledgments:** skip — no reply needed.

Keep tone terse, technical, and factual — zero performative agreement ("Great point!", "You're absolutely right!").

## 6. Done

Signal `ur-workertools status step-complete` (or `ur-workertools status request-human "<reason>"` if blocked).

## Guardrails

- Never skip bot comments; always skip `🤖` poller auto-replies.
- Never duplicate feedback already tracked by an existing child ticket.
- Never omit `repo comments link` for any comment in a merged group.
- Never use performative flattery in PR replies.

$ARGUMENTS
<!-- compiled-from: skills/specs/address-feedback.md@9bd256c03d36 profile:worker@6afed0c52191 -->
