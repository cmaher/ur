# Tea Command Reference and Extended Workflows

This guide provides extended command reference for `tea` operations when collaborating with a Gitea instance directly from the host.

## Output Discipline

Tea commands that query data support JSON output with `--output json` (or `-o json`). Always pass `--output json` on read operations for deterministic, machine-parseable output.

Examples:
```bash
tea pr ls --output json
tea pr view 42 --output json
tea pr checks 42 --output json
tea pr comments 42 --output json
tea pr review-comments 42 --output json
tea runs ls --output json
```

## Supported Pull Request Workflows

### Creating Pull Requests

Always specify `--title`. Optionally pass `--description`, `--base`, or `--head`:
```bash
tea pr create --title "feat: support Gitea webhooks" --description "Adds webhook handlers" --base master
```

### Reviewing Pull Requests

Tea supports structured reviews. Pass `--approve`, `--reject`, or `--comment` with a comment message:

```bash
# Approve with comment
tea pr review 42 --approve --comment "Looks good to me!"

# Request changes with comment
tea pr review 42 --reject --comment "Please add tests for the failure path."

# General review comment
tea pr review 42 --comment "Minor note on naming conventions."
```

### Thread Resolution

To resolve or reopen inline review comments:
```bash
# Mark resolved
tea pr resolve 42 1052

# Reopen thread
tea pr unresolve 42 1052
```

### Merging Pull Requests

Gitea supports four merge styles. Always specify the style explicitly to avoid interactive prompts:
- `merge`: Standard merge commit
- `rebase`: Rebase commits onto base branch
- `squash`: Squash commits into a single commit
- `rebase-merge`: Fast-forward rebase when possible, merge commit otherwise

```bash
tea pr merge 42 --style squash
```

Before merging:
1. Verify PR is open: `tea pr view 42 --output json`
2. Verify checks pass: `tea pr checks 42 --output json`

After merging:
1. Verify PR is closed: `tea pr view 42 --output json`

## Prohibited Operations

Do not run the following operations via `tea`:
- Authentication changes: `tea login`, `tea logout`, `tea auth`
- Administration: `tea admin`, `tea repo`, `tea org`, `tea migrate`
- Git helpers: `tea pr checkout`, `tea pr clean`, `tea clone`
- Credential flags: `--token`, `--password`, `--secret`, `--key`
- Insecure TLS: `--insecure`, `-k`, `--insecure-skip-tls-verify`
- Interactive commands without required flags
