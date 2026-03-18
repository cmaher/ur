---
name: push
description: Push changes to a branch, create a PR, and fix merge conflicts or CI failures — does NOT merge
---

1. **Branch check**: Run `git branch --show-current` to get the current branch name.
   - If on `main` or `master`, create and check out a new branch. Derive the branch name from the staged/unstaged changes (short, kebab-case, descriptive).
   - If already on a feature branch, check for a closed/merged PR (see step 4 pre-check below). If one exists, create a new branch off the main branch instead — derive the name from the changes, cherry-pick or re-apply the commits, and proceed on the new branch.
   - Otherwise, stay on the current feature branch.

2. **Commit**: Stage and commit the changes without asking for confirmation. Follow the repository's commit message conventions based on recent `git log`. Summarize all staged changes accurately.

3. **Pre-push verification**: Before pushing, run verification scripts if configured.
   - Read the `$UR_VERIFICATION_SCRIPTS` environment variable. It contains newline-separated shell commands.
   - If the variable is empty or unset, skip verification entirely.
   - **Noverify check**: If a ticket ID is available (from `$ARGUMENTS` or context), check whether verification should be skipped:
     - Run `ur ticket show <ticket-id> --output json` and inspect the `metadata` array for a key `noverify` with value `true`.
     - If `noverify` is set, skip all verification scripts and proceed to push.
   - **Run each command** in the variable as a separate shell command (e.g., `bash -c "<command>"`).
   - If ANY command fails (non-zero exit), you MUST fix the issues and re-run ALL verification commands until they all pass. Do NOT skip or ignore failures. There is no `--no-verify` flag available to you.
   - Only proceed to push after all verification commands pass.

4. **Push**: Push the branch to `origin` with `-u` to set upstream tracking.

5. **Pull Request**: Run `gh pr view --json url,state,number` to check if a PR already exists for this branch.
   - If no PR exists, or the existing PR is closed/merged, create a new one with `gh pr create`. Derive the title and body from the commit(s) on the branch. Use `--fill` as a starting point but write a clear summary.
   - If an open PR already exists, display its URL.

6. **Ticket metadata**: If a ticket ID is available (from `$ARGUMENTS` or context):
   - On **first push** (no upstream tracking existed before step 4), set the branch: `ur ticket set-meta <id> branch <branch-name> --output json`
   - After PR creation or discovery, set PR metadata:
     - `ur ticket set-meta <id> pr_number <number> --output json`
     - `ur ticket set-meta <id> pr_url <url> --output json`

7. **Merge conflicts**: If the push or PR creation fails due to merge conflicts, attempt to resolve them:
   - `git fetch origin main && git merge origin/main`
   - Resolve conflicts, commit, and push again.
   - If conflicts cannot be resolved automatically, report the conflicting files and stop.

8. **CI failures**: After pushing, check CI status with `gh pr checks`.
   - If checks are pending, report the PR URL and stop — do NOT wait.
   - If checks fail, read the failed logs with `gh run view <run-id> --log-failed`, fix the issues, commit, and push again. Retry up to 2 times.

**Important**: This skill does NOT merge the PR. It only pushes and creates/updates the PR.

$ARGUMENTS
