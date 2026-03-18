---
name: fix
description: Fix a failing ticket — pre-push verification, CI failures, or merge conflicts — invoked by workerd Fix RPC
---

Parse `$ARGUMENTS` for one required positional argument: `<ticket_id>`.

## 1. Read the ticket

Run `ur ticket show <ticket_id> --output json` to get the ticket details and metadata.

Extract the `fix_phase` value from the ticket's metadata array (key: `fix_phase`). Valid phases: `verify`, `ci`, `merge`.

## 2. Read failure details

Run `ur ticket list-activities <ticket_id> --output json` to get the ticket's activity log. Filter for activities where `source` equals `workflow` — these contain the failure output written by the workflow engine. Read the most recent workflow activities to understand what failed.

## 3. Fix based on phase

### Phase: `verify` (pre-push hook failure)

The pre-push hook (verification scripts) failed. The workflow activities contain the hook output showing which checks failed and why.

1. Read the failure output from the workflow activities carefully.
2. Identify the specific errors (lint failures, test failures, formatting issues, clippy warnings, etc.).
3. Fix each issue in the codebase.
4. Run the verification scripts to confirm the fixes work. Check the `$UR_VERIFICATION_SCRIPTS` environment variable for the commands to run.
5. Stage and commit the fixes.

### Phase: `ci` (CI check failures)

One or more CI checks failed after pushing. The workflow activities contain the names of the failed checks.

1. Read the failed check names from the workflow activities.
2. Investigate each failure using `gh run view <run-id> --log-failed` to get detailed logs. Use `gh pr checks --json name,state,conclusion,link` on the PR to find the run IDs.
3. Fix the underlying issues in the codebase.
4. Stage and commit the fixes.
5. Push the fixes: `git push`.

### Phase: `merge` (merge conflict)

A merge conflict occurred. The workflow activities contain details about the conflict.

1. Fetch the latest base branch: `git fetch origin master`.
2. Attempt to merge: `git merge origin/master`.
3. Resolve any conflicts by examining the conflicting files and making the correct choices.
4. Complete the merge commit.

## 4. Signal completion

When you have successfully fixed the issue:

```
workertools agent done
```

If you cannot fix the issue and need human intervention, explain why and run:

```
workertools agent request-human "description of why human help is needed"
```

You MUST run one of these two commands before stopping. Do not stop without signaling.

$ARGUMENTS
