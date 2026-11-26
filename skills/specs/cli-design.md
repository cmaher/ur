# cli-design

- **Targets:** worker, local
- **Trigger:** Designing, building, or reviewing a CLI tool that AI agents will invoke.

## Core
- **Core thesis:** human DX optimizes for discoverability and forgiveness; agent DX optimizes for predictability, structured I/O, context-window discipline, and defense-in-depth. **Agents hallucinate and parse literally — design for both.**
- **Structured I/O & stream discipline:**
  - Machine-readable output first: `--output json` (or `ndjson` for streams/pagination), env var fallback (`OUTPUT_FORMAT=json`), or auto-JSON when stdout is not a TTY.
  - Strict stream separation: stdout = pure data (valid JSON when requested); stderr = progress, warnings, human diagnostics, and structured error JSON envelopes (`{ "ok": false, "error": { "code", "message", "hint" } }`).
  - Support structured input (`--json '{...}'` or `--input -` on stdin) alongside ergonomic human flags for complex payloads so agents don't battle shell quoting across 15 flags.
- **Non-interactive & deterministic by default:**
  - Never hang waiting for a prompt when stdin is not a TTY; fail fast with a clear error listing the missing required flag.
  - Stable, documented exit-code taxonomy (e.g., `0` ok, `2` unreachable/transient, `3` not found, `5` invalid input, `6` permission/policy, `10` timeout) so callers branch on exit code, not regexes over English text.
  - Explicit idempotency / retry semantics where applicable.
- **Runtime discoverability (replace stale static prose):**
  - Self-describing commands (`--describe`, `schema <cmd>`, or `--help --json`) returning accepted flags, payload schemas, exit codes, and response shapes as JSON.
  - Pair the binary with a concise agent skill/doc encoding invariants `--help` cannot convey (ordering, safety rules, required field masks).
- **Context-window discipline:**
  - Field masks (`--fields a,b,c`) and bounded pagination/limits (`--limit`, cursor/NDJSON streaming) so list/get calls don't dump megabytes into agent context.
  - Deterministic truncation metadata (`truncated: true`, `remaining: N`, `next_cursor`) rather than silently dropping data.
- **Input hardening (treat agent input as untrusted):**
  - Validate early, fail loudly before side effects: reject path traversal (`..`, resolved outside allowed root), ASCII control chars (`< 0x20`), embedded query/fragment injection (`id?foo=bar`), double-encoding (decode before validating), and unencoded path segments.
  - Fuzz/unit-test with agent-typical hallucinations (wrong ID formats, empty strings, relative escapes, trailing whitespace).
- **Safety rails:**
  - `--dry-run` for mutating/destructive operations (validates inputs + auth + target state and prints planned diff/action without executing).
  - Non-interactive auth via env vars / credential files / host-managed auth (never browser OAuth redirects mid-command; never accept secrets as visible CLI flags when env/stdin/file works).
  - Treat external/user-controlled response payloads as untrusted data (clear framing or sanitization when returning untrusted prose that could contain prompt injection).
- **Incremental adoption checklist (priority order when upgrading an existing CLI):**
  1. `--output json` + strict stdout/stderr separation + structured errors/exit codes
  2. Non-interactive fail-fast + input hardening
  3. Field masks / limits / pagination bounds
  4. `--dry-run` on mutations
  5. Runtime schema introspection (`--describe` / `schema`)
  6. Concise agent skill doc covering non-obvious invariants

## Guardrails
- Never mix log lines or ANSI color codes into `--output json` stdout.
- Never hang on interactive confirmation prompts without a TTY.
- Never return unbounded API/DB dumps without limits or field-selection support.
- Never trust agent-supplied paths, IDs, or resource names without boundary validation.
<!-- REVIEW: Reworked from the old Google-Workspace-centric draft: added stdout/stderr separation, exit-code taxonomy, non-interactive fail-fast, and truncation metadata; trimmed the generic MCP section. Let me know what else you want added, removed, or emphasized (e.g., Rust/clap conventions, MCP, or specific `ur` CLI lessons). -->
