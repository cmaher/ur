---
name: gitea
description: Use when working with Gitea repositories, pull requests, PR reviews or comments, CI/Actions checks, or merging pull requests on Gitea
---

# Gitea Collaboration

Use `git` for all repository, branch, commit, rebase, and push operations, and `tea` exclusively for Gitea forge operations (pull requests, reviews, comments, CI checks, and merging).

## Strict Boundary Rules

1. **Never use `gh`** (not even as fallback) — it targets GitHub, not Gitea.
2. **Use `git` for repo and branch ops:** always use `git fetch`, `git checkout`, `git branch`, `git commit`, `git rebase`, `git push`. Never run `tea clone`, `tea pr checkout`, or `tea pr clean`.
3. **Never manage auth or pass secrets:** never run `tea login`, `tea logout`, `tea auth`, or pass `--token`, `--password`, `--secret`, or `--key`.
4. **Never disable TLS verification:** never pass `--insecure`, `-k`, or `--insecure-skip-tls-verify`.
5. **Always run non-interactively:** supply required flags explicitly (`--title` and `--description` on `tea pr create`; `--style <merge|rebase|squash|rebase-merge>` on `tea pr merge`).
6. **Always use `--output json` on reads:** `tea pr ls`, `tea pr view`, `tea pr checks`, `tea pr comments`, `tea pr review-comments`, `tea runs ls`.

## Quick Reference

| Action | Command |
| --- | --- |
| List open PRs | `tea pr ls --output json` |
| View PR details/state | `tea pr view <id> --output json` |
| View PR diff | `tea pr diff <id>` |
| Check CI status | `tea pr checks <id> --output json` |
| List workflow runs | `tea runs ls --output json` / `tea actions runs ls --output json` |
| Create PR | `tea pr create --title "<title>" --description "<body>"` |
| Edit / close / reopen PR | `tea pr edit <id> --title "..."` · `tea pr close <id>` · `tea pr reopen <id>` |
| Review PR | `tea pr review <id> [--approve\|--reject] --comment "<text>"` |
| List inline / issue comments | `tea pr review-comments <id> --output json` · `tea pr comments <id> --output json` |
| Reply / resolve thread | `tea pr comment <id> "<text>"` · `tea pr resolve <id> <comment-id>` |
| Merge PR | `tea pr merge <id> --style <merge\|rebase\|squash\|rebase-merge>` |

Extended command examples: [tea-workflows.md](references/tea-workflows.md).

## Core Workflows

1. **Create or update a PR:** commit and push via `git` → check existing PRs (`tea pr ls --output json`) → create (`tea pr create --title ... --description ...`) → re-verify (`tea pr view <id> --output json`).
2. **Inspect PR & CI:** `tea pr view <id> --output json`, `tea pr checks <id> --output json`, `tea pr diff <id>`, and `tea runs ls --output json` / `tea actions runs ls --output json`.
3. **Review & triage comments:** read `tea pr review-comments <id> --output json` and `tea pr comments <id> --output json` → edit/commit/push via `git` → reply (`tea pr comment`), resolve (`tea pr resolve <id> <comment-id>`), submit verdict (`tea pr review <id> [--approve|--reject] --comment "<text>"`) → re-verify (`tea pr view <id> --output json`).
4. **Merge a PR (mandatory pre- and post-checks):** verify PR is open (`tea pr view <id> --output json`) AND CI checks pass (`tea pr checks <id> --output json`) → merge with explicit `--style` (`merge`, `rebase`, `squash`, or `rebase-merge`) → re-verify merged state (`tea pr view <id> --output json`).

## Guardrails

- Never invoke `gh` on a Gitea repository.
- Never omit `--style` on `tea pr merge` or `--title` on `tea pr create` (hangs or violates policy).
- Never assume a mutation succeeded without re-reading `tea pr view <id> --output json`.
- Never pass credentials on the command line or weaken TLS verification.
<!-- compiled-from: skills/specs/gitea.md@e1888832d1d7 profile:worker@6afed0c52191 -->
