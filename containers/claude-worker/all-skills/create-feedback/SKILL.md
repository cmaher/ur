---
name: create-feedback
description: Use when creating follow-up tickets from PR review comments
---

Parse `$ARGUMENTS` for two required positional arguments: `<epic_id>` and `<pr_number>`.

## 1. Fetch the ticket

Run `ur ticket show <epic_id> --output json` to get the ticket's details. Record its `project` and `title`.

If the ticket's type is `epic`, also run `ur ticket list --epic <epic_id> --output json` to see existing child tickets — avoid creating duplicates of feedback already tracked.

## 2. Fetch PR review comments

Run `gh api repos/{owner}/{repo}/pulls/<pr_number>/comments --paginate` to fetch all inline review comments on the PR.

- Parse the JSON array of review comment objects.
- Extract the `body`, `path`, `line`, `html_url`, and `user.login` fields from each comment.
- Skip bot comments (users ending in `[bot]`).

Also fetch top-level PR comments (conversation comments, not inline reviews):

```
gh api repos/{owner}/{repo}/pulls/<pr_number>/comments --paginate
gh api repos/{owner}/{repo}/issues/<pr_number>/comments --paginate
```

Use `gh api repos/{owner}/{repo}/pulls/<pr_number> --jq '.head.repo.full_name'` (or parse from `gh pr view <pr_number> --json url`) to determine `{owner}/{repo}`.

If there are no actionable review comments after filtering, skip to step 4.

## 3. Create child tickets from review comments

For each actionable review comment (or group of related comments on the same topic), create a child ticket under the existing epic:

```
ur ticket create "<short summary of the feedback>" \
  --parent <epic_id> \
  --priority 2 \
  --body "PR Feedback from PR #<pr_number>

Reviewer: @<user>
PR: #<pr_number>
File: <path>:<line>
Comment: <html_url>

---

<full comment body>" \
  --output json
```

Guidelines for grouping:
- Multiple comments about the same concern should be merged into a single ticket.
- Each ticket title should be a concise, actionable description (e.g., "Add error handling for edge case X", "Rename function Y for clarity").
- When merging multiple comments into one ticket, include all PR context entries (each with their own file, line, URL, and comment body).

For top-level PR comments (from the issues endpoint), omit the `File:` and line fields since they are not file-specific:

```
ur ticket create "<short summary of the feedback>" \
  --parent <epic_id> \
  --priority 2 \
  --body "PR Feedback from PR #<pr_number>

Reviewer: @<user>
PR: #<pr_number>
Comment: <html_url>

---

<full comment body>" \
  --output json
```

## 4. Done

**REQUIRED: Signal completion by running `workertools step-complete` in bash when all work is done.** The system will not advance until this signal is sent.

If you cannot complete the task, run `workertools agent request-human "<reason>"` and stop.

$ARGUMENTS
