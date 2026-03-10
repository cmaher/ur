---
description: Set up the bacon background checker for a Rust project.
---

## Instructions

### Phase 1: Verification

Check that `Cargo.toml` exists in the project root. If not, abort with "Not a Rust project."

### Phase 2: Installation

Run `bacon --version` to check if bacon is installed.

If missing:
1. Report: "Installing bacon via cargo..."
2. Run: `cargo install --locked bacon`
3. Verify installation succeeded.

### Phase 3: Configuration

1. Run `bacon --init` to create `bacon.toml` if it doesn't exist.
2. Check `bacon.toml` for a job named `[jobs.ai]`. If missing, append:

```toml
[jobs.ai]
command = ["cargo", "check", "--color", "never", "--message-format", "short"]
need_stdout = true
watch = ["src", "crates"]
```

3. Check `bacon.toml` for an `[exports.locations]` section. If missing, append:

```toml
[exports.locations]
auto = true
path = ".bacon-locations"
line_format = "{kind} {path}:{line}:{column} {message}"
```

4. Check `.gitignore` for `.bacon-locations`. If missing, append it.

### Phase 4: Documentation

Append the following to the project's `CLAUDE.md` if not already present:

```
- **Rust Verification (Bacon)**:
  - Bacon runs as a **persistent background watcher** — the user starts it once in a terminal. Do NOT launch `bacon` yourself.
  - Read `.bacon-locations` to get current diagnostics (errors/warnings from the last compile). This file is auto-updated by bacon's export-locations feature.
  - If `.bacon-locations` doesn't exist or is empty, bacon may not be running. Fall back to `cargo check --message-format short 2>&1`.
  - If you need to see only errors (no warnings), filter lines starting with `error` from `.bacon-locations`.
```

### Phase 5: Completion

Report:
- Whether bacon was installed or already present.
- That the `ai` job and `exports.locations` have been configured in `bacon.toml`.
- That agents should read `.bacon-locations` instead of launching bacon.
