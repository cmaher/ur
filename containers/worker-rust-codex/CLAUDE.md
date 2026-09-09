# worker-rust-codex (Container Image)

Extends `ur-worker-codex:latest` with build dependencies for Rust projects — the codex twin of
`containers/worker-rust-claude/`. Cargo and bacon run on the host via hostexec, not inside the
container.

- Build context is `containers/worker-rust-codex/` — all files copied into the image must
  live here
- Image is tagged `ur-worker-rust-codex:latest` by convention — the directory name matches
  the tag
- Inherits everything from `ur-worker-codex` (codex CLI, tmux entrypoint, worker binaries)
- Rust toolchain (cargo, bacon, etc.) runs on the host via hostexec shims — no mise or local
  toolchain in the container. Unlike `worker-rust-claude`, this context deliberately carries
  no `mise.toml` / `install-mise.sh`: this `Dockerfile` never references them, so staging a
  generated copy here would be dead weight. Don't add them "for symmetry" — the claude
  context's copies are unreferenced too
- At runtime, `workerd` creates shims for hostexec commands (git, gh, cargo, bacon, etc.)
- `AgentType::Codex.fallback_image()` resolves to `ur-worker-rust-codex:latest` — a no-project
  codex launch lands here, so this image existing and healthy is load-bearing, not optional
- Entrypoint mirrors `worker-rust-claude/entrypoint.sh`: `workerd init` (synchronous), then the
  background processes (`cargo sweep`, `bacon --headless`), then `exec workerd daemon`.
  Background-process launch stays in the image's entrypoint rather than in `workerd` itself,
  keeping `workerd` image-agnostic
