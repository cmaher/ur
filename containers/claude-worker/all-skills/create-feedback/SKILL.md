---
name: create-feedback
description: Use when creating follow-up tickets from PR review comments
---

Parse `$ARGUMENTS` for two required positional arguments: `<epic_id>` and `<pr_number>`.

## 1. Fetch the ticket

Run `ur ticket show <epic_id> --output json` to get the ticket details. Record its `project`, `title`, and `priority`.

If the ticket has children, also run `ur ticket list --parent <epic_id> --output json` to see existing child tickets — avoid creating duplicates of feedback already tracked.

## 2. Fetch PR review comments

Run `gh api repos/{owner}/{repo}/pulls/<pr_number>/comments --paginate` to fetch all inline review comments on the PR.

- Parse the JSON array of review comment objects.
- Extract the `body`, `path`, `line`, `html_url`, `id`, and `user.login` fields from each comment.
- Do NOT skip bot comments — bot feedback (CI failures, linters) is actionable.

Also fetch top-level PR comments (conversation comments, not inline reviews):

```
gh api repos/{owner}/{repo}/pulls/<pr_number>/comments --paginate
gh api repos/{owner}/{repo}/issues/<pr_number>/comments --paginate
```

Use `gh api repos/{owner}/{repo}/pulls/<pr_number> --jq '.head.repo.full_name'` (or parse from `gh pr view <pr_number> --json url`) to determine `{owner}/{repo}`.

## 3. Triage comments

Classify each comment into one of two buckets:

**Ticket-worthy** — actionable feedback that requires code changes: bug reports, missing error handling, refactoring requests, performance concerns, substantive suggestions. Group related comments about the same concern into a single ticket.

**Reply-only** — comments that deserve a response but not a ticket: questions, nits, style observations, praise, discussion. The agent will reply to these directly on the PR.

If there are no comments in either bucket, skip to step 6.

## 4. Create child tickets and link comments

For each ticket-worthy group, create a child ticket:

```
ur ticket create "<short summary of the feedback>" \
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

Use the parent ticket's priority value recorded in step 1.

After creating each ticket, link every source comment in the group:

```
workertools repo comments link --pr <pr_number> --comment <comment_id> --ticket <ticket_id>
```

One call per source comment. The server uses these links to post automatic "Tracking in `ur-xyz`" replies.

Guidelines for grouping:
- Multiple comments about the same concern should be merged into a single ticket.
- Each ticket title should be a concise, actionable description (e.g., "Add error handling for edge case X", "Rename function Y for clarity").
- When merging multiple comments into one ticket, include all PR context entries (each with their own file, line, URL, and comment body).

For top-level PR comments (from the issues endpoint), omit the `File:` and line fields since they are not file-specific.

## 5. Reply to non-ticket comments

For each reply-only comment, post an in-thread reply:

```
workertools repo comments reply --pr <pr_number> <comment_id> "<message>"
```

Exercise judgment:
- **Nits**: Address them — fix the issue or explain why not. Be specific.
- **Questions**: Answer directly. If you don't know, say so.
- **Discussion**: Engage substantively. Push back with technical reasoning when appropriate.
- **Praise/acknowledgment**: Skip — no reply needed.

Tone: terse, technical. No performative agreement ("Great point!", "You're absolutely right!"). State facts, reference code, move on.

## 6. Done

**REQUIRED: Signal completion by running `workertools step-complete` in bash when all work is done.** The system will not advance until this signal is sent.

If you cannot complete the task, run `workertools agent request-human "<reason>"` and stop.

$ARGUMENTS
