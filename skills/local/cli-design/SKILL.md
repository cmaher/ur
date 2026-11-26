---
name: cli-design
description: Use when designing, building, or reviewing a CLI tool that AI agents will invoke
---

# Designing CLIs for AI Agents

Human DX optimizes for discoverability and forgiveness. Agent DX optimizes for predictability, structured I/O, context-window discipline, and defense-in-depth. **Agents hallucinate and parse literally — design for both.**

## Core Design Principles

### 1. Structured I/O & Stream Discipline
- **Machine-readable output first:** support `--output json` (and `ndjson` for streaming/pagination), `OUTPUT_FORMAT=json`, or auto-JSON when stdout is not a TTY.
- **Strict stream separation:** stdout emits **pure data only** (valid JSON when requested, zero logs or ANSI color codes); stderr carries progress, warnings, human diagnostics, and structured error envelopes (`{"ok":false,"error":{"code":"...","message":"...","hint":"..."}}`).
- **Structured input alongside human flags:** accept `--json '{...}'` or `--input -` (stdin) for complex payloads so agents do not fight shell escaping across 15 bespoke flags.

### 2. Non-Interactive & Deterministic by Default
- **Fail fast without a TTY:** never hang waiting for interactive confirmation or missing prompts when stdin is not a TTY; exit with an error naming the missing required flag.
- **Stable exit-code taxonomy:** document and enforce numeric exit codes so callers branch on exit status rather than scraping English prose (e.g., `0` ok, `2` unreachable/transient, `3` not found, `5` invalid input, `6` permission/policy, `10` timeout).
- **Explicit idempotency & retry semantics:** make retries safe or state clearly when an operation is non-idempotent.

### 3. Runtime Discoverability
- **Self-describing commands:** expose `--describe`, `schema <cmd>`, or `--help --json` returning accepted flags, payload schemas, exit codes, and response shapes as JSON so docs do not go stale in prompt context.
- **Pair with a concise agent skill:** document cross-command invariants `--help` cannot convey (ordering, safety rails, required field masks).

### 4. Context-Window Discipline
- **Field masks & bounded pagination:** support `--fields a,b,c`, `--limit N`, and cursor or NDJSON pagination so list/get calls never dump megabytes into agent context.
- **Explicit truncation metadata:** whenever output is bounded, return deterministic metadata (`truncated: true`, `remaining: N`, `next_cursor`) rather than silently dropping items.

### 5. Input Hardening (Treat Agent Input as Untrusted)
Validate early and fail loudly before any side effects:

| Vector | Example | Defense |
| --- | --- | --- |
| Path traversal | `../../.ssh/id_rsa` | Canonicalize/validate paths stay inside allowed root; reject `..` |
| Control characters | Bytes `< 0x20` | Reject ASCII control chars in string inputs |
| Query/fragment injection | `itemId?fields=all` | Reject query/fragment chars (`?`, `#`) in resource IDs |
| Double encoding | `%2e%2e` | Decode before validating |
| Unencoded path segments | Spaces/slashes in names | Reject raw path separators in IDs; percent-encode path segments |

Fuzz and unit-test with agent-typical hallucinations: wrong ID shapes, empty strings, relative escapes, trailing whitespace.

### 6. Safety Rails
- **`--dry-run` on mutations:** validate inputs, auth, and target state and print the planned diff/action without executing side effects.
- **Non-interactive auth:** accept credentials via env vars, credential files, or host-managed auth — never trigger browser OAuth mid-command and never accept secrets in visible CLI flags when env/stdin/file works.
- **Untrusted external payloads:** frame or sanitize external/user-controlled response prose so embedded prompt injections cannot hijack the calling agent.

## Incremental Adoption Order

1. `--output json` + strict stdout/stderr separation + structured error JSON & exit codes
2. Non-interactive fail-fast + boundary input hardening
3. Field masks, `--limit`, pagination bounds, and truncation metadata
4. `--dry-run` on mutating/destructive commands
5. Runtime schema introspection (`--describe` / `schema`)
6. Concise agent skill doc encoding non-obvious invariants

## Guardrails

- Never mix logs or ANSI escape codes into `--output json` stdout.
- Never hang on interactive prompts when stdin is not a TTY.
- Never return unbounded API or DB dumps without limits or field selection.
- Never trust agent-supplied paths, IDs, or resource names without boundary validation.
<!-- compiled-from: skills/specs/cli-design.md@e0b8e44d60d3 profile:local@daa4419e0589 -->
