---
name: create-feedback
description: Create follow-up tickets from PR review comments — invoked by workerd CreateFeedbackTickets RPC
---

Parse `$ARGUMENTS` for two required positional arguments: `<ticket_id>` and `<pr_number>`.

## 1. Fetch the original ticket

Run `ur ticket show <ticket_id> --output json` to get the original ticket's details. Record its `parent_id` and `project`.

## 2. Fetch PR review comments

Run `gh api repos/{owner}/{repo}/pulls/<pr_number>/comments --paginate` to fetch all review comments on the PR.

- Parse the JSON array of review comment objects.
- Extract the `body`, `path`, `line`, and `user.login` fields from each comment.
- Skip bot comments (users ending in `[bot]`).
- If there are no actionable review comments, skip to step 5.

Also fetch top-level PR comments (conversation comments, not inline reviews):

```
gh api repos/{owner}/{repo}/pulls/<pr_number>/comments --paginate
gh api repos/{owner}/{repo}/issues/<pr_number>/comments --paginate
```

Use `gh api repos/{owner}/{repo}/pulls/<pr_number> --jq '.head.repo.full_name'` (or parse from `gh pr view <pr_number> --json url`) to determine `{owner}/{repo}`.

## 3. Create follow-up epic

Create a follow-up epic as a **sibling** of the original ticket (same parent):

```
ur ticket create "Follow-up: <original_title>" \
  --type epic \
  --parent <original_parent_id> \
  --priority 1 \
  --body "Follow-up items from PR #<pr_number> review of <ticket_id>" \
  --follow-up <ticket_id> \
  --output json
```

This creates the epic and automatically adds a `follow_up` edge to the original ticket. Record the new epic's ID as `<followup_epic_id>`.

## 4. Create child tickets from review comments

For each actionable review comment (or group of related comments on the same topic), create a child ticket under the follow-up epic:

```
ur ticket create "<short summary of the feedback>" \
  --parent <followup_epic_id> \
  --priority 2 \
  --body "<full comment body>\n\nSource: <path>:<line> by @<user>" \
  --output json
```

Guidelines for grouping:
- Multiple comments about the same concern should be merged into a single ticket.
- Each ticket title should be a concise, actionable description (e.g., "Add error handling for edge case X", "Rename function Y for clarity").
- Include the file path and line number in the ticket body for context.

## 5. Update lifecycle status

Transition the original ticket's lifecycle status to `feedback_resolving`:

```
ur ticket update <ticket_id> --lifecycle feedback_resolving --output json
```

This signals that the original ticket now has feedback being addressed.

## 6. Signal completion

When you have finished creating feedback tickets (or determined there are none):

```
workertools agent done
```

If you cannot complete the task and need human intervention:

```
workertools agent request-human "description of why human help is needed"
```

You MUST run one of these two commands before stopping. Do not stop without signaling.

$ARGUMENTS
