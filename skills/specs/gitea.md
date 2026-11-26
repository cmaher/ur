# gitea

- **Targets:** worker, local
- **Trigger:** Working with Gitea repositories, pull requests, PR reviews/comments, CI/Actions checks, or merging on Gitea.
- **Assets:** `references/tea-workflows.md`

## Core
- **Division of labor:** `git` handles all repository, branch, commit, rebase, and push operations; `tea` handles all Gitea forge operations (PRs, reviews, comments, CI checks, merges).
- **Strict boundary rules:**
  - **Never use `gh`** (even as fallback) — it targets GitHub, not Gitea.
  - **Never use `tea clone`, `tea pr checkout`, or `tea pr clean`** — use `git fetch` / `git checkout` / `git branch`.
  - **Never manage auth or pass secrets:** never run `tea login`, `tea logout`, `tea auth`, or pass `--token`, `--password`, `--secret`, `--key`.
  - **Never disable TLS verification:** no `--insecure`, `-k`, or `--insecure-skip-tls-verify`.
  - **Always non-interactive:** pass required flags explicitly (`--title` + `--description` on `tea pr create`; `--style <merge|rebase|squash|rebase-merge>` on `tea pr merge`).
  - **Always `--output json` on reads:** `tea pr ls`, `tea pr view`, `tea pr checks`, `tea pr comments`, `tea pr review-comments`, `tea runs ls`.
- **Core workflows:**
  - **Create/update PR:** git commit & push → `tea pr ls --output json` (check existing) → `tea pr create --title ... --description ...` → verify via `tea pr view <id> --output json`.
  - **Inspect PR & CI:** `tea pr view`, `tea pr checks`, `tea pr diff`, `tea runs ls` / `tea actions runs ls`.
  - **Review & triage comments:** read `tea pr review-comments` + `tea pr comments` → edit/commit/push via `git` → reply (`tea pr comment`), resolve (`tea pr resolve <pr> <comment-id>`), submit review (`tea pr review --approve|--reject --comment`) → re-verify via `tea pr view`.
  - **Merge PR (mandatory pre- & post-checks):** verify open (`tea pr view`) AND CI passing (`tea pr checks`) → merge with explicit `--style` (`merge|rebase|squash|rebase-merge`) → re-verify merged state (`tea pr view`).
- Link extended command examples in `references/tea-workflows.md`.

## Guardrails
- Never invoke `gh` on a Gitea repo.
- Never omit `--style` on `tea pr merge` or `--title` on `tea pr create` (hangs or fails policy).
- Never assume a mutation succeeded without re-reading `tea pr view <id> --output json`.
- Never pass credentials on the command line or weaken TLS.
