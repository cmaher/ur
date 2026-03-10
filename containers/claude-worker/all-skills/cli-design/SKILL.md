---
name: cli-design
description: Use when designing, building, or reviewing a CLI tool that AI agents will invoke — covers machine-readable output, input hardening, schema introspection, context window discipline, and safety rails
---

# Designing CLIs for AI Agents

Human DX optimizes for discoverability and forgiveness. Agent DX optimizes for predictability and defense-in-depth. **Agents hallucinate. Build like it.**

## Core Principles

### 1. Raw JSON Payloads Over Bespoke Flags

Support both human-friendly flags AND structured JSON input. Agents need to pass full API payloads without mapping to dozens of flags.

```bash
# Human path
mycli create --title "My Doc" --locale en --timezone UTC

# Agent path — single structured input
mycli create --json '{"title": "My Doc", "locale": "en", "timezone": "UTC"}'
```

For output: `--output json`, `OUTPUT_FORMAT=json` env var, or default to NDJSON when stdout is not a TTY.

### 2. Schema Introspection Replaces Documentation

Static docs consume token budget and go stale. Make the CLI self-describing at runtime:

```bash
mycli schema drive.files.list   # Returns params, request body, response types as JSON
mycli describe create           # Or --help --json for any command
```

This is most valuable for API-backed CLIs, but `--describe` or `--help --json` works universally.

### 3. Context Window Discipline

Agents pay per token and lose reasoning capacity with every irrelevant field.

- **Field masks**: `--params '{"fields": "files(id,name,mimeType)"}'` or `--fields id,name,mimeType`
- **NDJSON pagination**: One JSON object per page for stream processing without buffering entire arrays

Skill files / agent instructions should say: "ALWAYS use field masks when listing or getting resources."

### 4. Input Hardening Against Hallucinations

The agent is not a trusted operator. Agents hallucinate differently from human typos:

| Attack Vector | Example | Defense |
|---|---|---|
| Path traversal | `../../.ssh/id_rsa` | Validate output dirs, reject `..` |
| Control characters | Invisible chars below ASCII 0x20 | Reject control chars in all string inputs |
| Embedded query params | `fileId?fields=name` | Strip query strings from resource IDs |
| Double encoding | `%2e%2e` → `..` after decode | Decode first, then validate |
| URL path segments | Special chars in filenames | Encode path segments properly |

Validate early, reject loudly. Fuzz with agent-typical mistakes during testing.

### 5. Ship Agent Skills, Not Just Commands

Agents learn through injected context files, not `--help` text. Ship skill/context files with YAML frontmatter encoding invariants agents cannot intuit:

- "Always use `--dry-run` for mutating operations"
- "Always confirm with user before write/delete commands"
- "Add `--fields` to every list call"

### 6. Multi-Surface Architecture

One binary, multiple agent surfaces:

- **MCP (Model Context Protocol)**: Expose commands as JSON-RPC tools over stdio — eliminates shell escaping and argument parsing ambiguity
- **Environment variables**: `MY_CLI_TOKEN`, `MY_CLI_CREDENTIALS_FILE` for credential injection without browser redirects
- **Extensions**: Native capability installation for agent platforms

All surfaces should derive from the same source of truth (API schema, discovery document, etc.).

### 7. Safety Rails: Dry-Run and Response Sanitization

- **`--dry-run`**: Validate requests locally before API invocation. Critical for create/update/delete.
- **`--sanitize`**: Filter API responses for prompt injection before returning to agent. Malicious content in data (e.g. email bodies with injected instructions) can hijack agent behavior.

## Implementation Order

Add these incrementally — no rewrite required:

1. `--output json` — machine-readable baseline
2. Input validation — reject control chars, path traversals, embedded params
3. `--describe` / schema command — runtime introspection
4. Field masks / `--fields` — context window protection
5. `--dry-run` — validation before mutation
6. Skill files — explicit invariant documentation for agents
7. MCP surface — typed JSON-RPC for API-backed CLIs

## Quick Reference

| Concern | Human CLI | Agent CLI |
|---|---|---|
| Input | Flat flags | Structured JSON payload |
| Output | Pretty tables | JSON / NDJSON |
| Docs | Man pages, `--help` | Schema introspection, skill files |
| Errors | Friendly messages | Structured error JSON with codes |
| Auth | Browser OAuth flow | Env vars, service accounts |
| Safety | Confirmation prompts | `--dry-run`, input validation, response sanitization |
| Pagination | "Next page? y/n" | NDJSON streaming |

## Common Mistakes

- **Omitting `--output json`**: The single highest-impact addition. Do this first.
- **Trusting agent input**: Agents hallucinate paths, IDs, and parameters. Validate everything.
- **Returning entire API responses**: Bloats context window and degrades reasoning. Support field masks.
- **Relying on `--help` text for agents**: Agents work from injected context, not interactive help. Ship skill files.
- **No `--dry-run`**: Agents should validate before mutating. Without this, mistakes are irreversible.

$ARGUMENTS
