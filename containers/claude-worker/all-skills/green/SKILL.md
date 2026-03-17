---
description: Use when a PR needs to pass CI and be merged — monitors check status, fixes failures, and squash-merges when green
---

## Instructions

Parse `$ARGUMENTS` for an optional sleep interval in seconds (default: 60).

### 1. Find the PR

Run `gh pr view --json number,url,headRefName` to get the PR for the current branch.

- If no PR exists, invoke `/push` to create one, then re-run `gh pr view`.

### 2. Check CI status

Run `gh pr checks` to see all check statuses. Classify the overall result:

- **All passed** — go to step 3.
- **Any pending or in-progress** — sleep for the configured interval, then re-check.
- **Any failed** — go to step 4.

### 3. Merge

Run `gh pr merge --squash --delete-branch`. Report the merged PR URL.

- If merge fails due to conflicts, stop and inform me.

### 4. Fix failures

For each failed check:

1. Get the run ID from `gh pr checks --json name,state,conclusion,link` and extract the GitHub Actions run ID from the link.
2. Read the failed logs: `gh run view <run-id> --log-failed`.
3. If logs aren't available (external CI), read the check output and try to reproduce locally.

Fix all failures before pushing — multiple fixes in one push saves CI cycles.

Stage, commit, and push to the current branch. Then go back to step 2.

$ARGUMENTS
