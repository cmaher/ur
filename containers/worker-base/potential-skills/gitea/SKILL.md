---
name: gitea
description: Use when working with Gitea repositories, managing Gitea pull requests, reviewing code on Gitea, replying to or resolving Gitea comments, inspecting CI/actions status on Gitea, or merging Gitea pull requests
---

# Gitea Collaboration Skill

## Overview

Collaborate with Gitea repositories using `git` for repository and commit operations and `tea` for forge collaboration (pull requests, code review, comments, CI status, and merging).

## Strict Boundary Rules

1. **Never use `gh`**: Do NOT run `gh` or suggest `gh` as fallback. GitHub CLI commands will fail or target the wrong platform. All forge actions must use `tea`.
2. **Use `git` for repository & branch operations**: Always use standard `git` commands (`git fetch`, `git checkout -b`, `git commit`, `git push`, `git rebase`). Never use `tea clone`, `tea pr checkout`, or `tea pr clean`.
3. **Host-managed authentication**: Tea authentication and tokens are managed on the host. NEVER run `tea login`, `tea logout`, `tea auth`, or pass credentials (`--token`, `--password`, `--secret`, `--key`) in arguments.
4. **No insecure TLS**: Never pass `--insecure`, `-k`, or `--insecure-skip-tls-verify`.
5. **Non-interactive execution**: All commands must be non-interactive. Supply required flags directly (e.g. `--title` for `tea pr create`, `--style` for `tea pr merge`). Never run commands that prompt for user input.
6. **JSON output for read operations**: Use `--output json` on read commands (`tea pr ls`, `tea pr view`, `tea pr checks`, `tea pr comments`, `tea pr review-comments`, `tea runs ls`). Parse the structured JSON response.

## Quick Reference

| Action | Command | Notes |
|---|---|---|
| List pull requests | `tea pr ls --output json` | Returns open PRs |
| View pull request | `tea pr view <index> --output json` | PR details and status |
| View PR diff | `tea pr diff <index>` | Patch/diff output |
| Check CI status | `tea pr checks <index> --output json` | Commit check states |
| Create pull request | `tea pr create --title "<title>" --description "<body>"` | Requires `--title` |
| Edit pull request | `tea pr edit <index> --title "<new-title>"` | Edit title/body |
| Close / Reopen PR | `tea pr close <index>` / `tea pr reopen <index>` | Update PR state |
| Review: Approve | `tea pr review <index> --approve --comment "<text>"` | Approve PR |
| Review: Request Changes | `tea pr review <index> --reject --comment "<text>"` | Request changes |
| Review: Comment | `tea pr review <index> --comment "<text>"` | Review comment |
| List review comments | `tea pr review-comments <index> --output json` | Inline thread comments |
| List conversation comments | `tea pr comments <index> --output json` | Top-level PR comments |
| Add PR comment | `tea pr comment <index> "<text>"` | Post comment |
| Resolve review thread | `tea pr resolve <index> <comment-id>` | Mark thread resolved |
| Unresolve review thread | `tea pr unresolve <index> <comment-id>` | Reopen thread |
| Merge pull request | `tea pr merge <index> --style <merge\|rebase\|squash\|rebase-merge>` | Explicit style required |
| View CI runs | `tea runs ls --output json` | Action run listings |

For extended reference and examples, see [tea-workflows.md](references/tea-workflows.md).

## Workflows

### 1. Create or Update a Pull Request

1. **Prepare changes with Git**:
   ```bash
   git checkout -b feature-branch
   git add <files>
   git commit -m "Description of changes"
   git push -u origin feature-branch
   ```

2. **Check for existing PR**:
   ```bash
   tea pr ls --output json
   ```

3. **Create PR if none exists**:
   ```bash
   tea pr create --title "Feature title" --description "Summary of changes"
   ```

4. **Verify state**:
   ```bash
   tea pr view <index> --output json
   ```

### 2. Inspect Pull Request and CI Checks

1. **Fetch PR details**:
   ```bash
   tea pr view <index> --output json
   ```
2. **Inspect CI status**:
   ```bash
   tea pr checks <index> --output json
   ```
3. **Inspect changes**:
   ```bash
   tea pr diff <index>
   ```

### 3. Review Feedback and Triage Comments

1. **Inspect comments and review threads**:
   ```bash
   tea pr review-comments <index> --output json
   tea pr comments <index> --output json
   ```
2. **Address feedback**:
   - Make code edits and commit via `git commit`.
   - Push updates via `git push origin <branch>`.
3. **Reply and resolve threads**:
   - Reply with explanation: `tea pr comment <index> "Addressed in commit <hash>"`
   - Resolve thread: `tea pr resolve <index> <comment-id>`
4. **Submit review verdict**:
   - Approve: `tea pr review <index> --approve --comment "LGTM"`
   - Or request changes: `tea pr review <index> --reject --comment "Please address feedback"`
5. **Re-read PR state**:
   ```bash
   tea pr view <index> --output json
   ```

### 4. Merge a Pull Request

1. **Pre-merge verification (MANDATORY)**:
   - Verify PR is open:
     ```bash
     tea pr view <index> --output json
     ```
   - Verify CI checks are passing:
     ```bash
     tea pr checks <index> --output json
     ```
2. **Execute merge with explicit style**:
   - Choose one of: `merge`, `rebase`, `squash`, `rebase-merge`.
   ```bash
   tea pr merge <index> --style squash
   ```
3. **Post-merge verification**:
   - Verify PR state has transitioned to closed/merged:
     ```bash
     tea pr view <index> --output json
     ```

### 5. Diagnosing CI or Action Failures

1. Check commit check status:
   ```bash
   tea pr checks <index> --output json
   ```
2. Check workflow runs:
   ```bash
   tea runs ls --output json
   tea actions runs ls --output json
   ```

## Common Mistakes

| Mistake | Why it Fails | Correct Approach |
|---|---|---|
| Using `gh` (`gh pr list`, `gh pr create`) | `gh` targets GitHub, not Gitea, and may be blocked | Use `tea` exclusively for forge actions |
| Using `tea pr checkout <id>` | Tea checkout mutates working tree outside git | Use `git fetch origin` and `git checkout <branch>` |
| Omitting `--style` on `tea pr merge` | Host policy blocks merge without explicit supported style | Always specify `--style <merge\|rebase\|squash\|rebase-merge>` |
| Using unsupported merge style (`fast-forward`, etc.) | Host policy only permits `merge`, `rebase`, `squash`, `rebase-merge` | Use one of the four supported styles |
| Omitting `--title` on `tea pr create` | Prompts interactively in terminal, which hangs | Always pass `--title` (and `--description`) |
| Passing credentials (`--token`, `--password`) | Host policy rejects secrets in command arguments | Rely on host-configured credentials |
| Passing `--insecure` or `-k` | Insecure TLS is prohibited | Trust system certificates |
| Forgetting to re-verify state after mutation | Assumes mutation succeeded without checking server state | Always re-run `tea pr view <index> --output json` |
