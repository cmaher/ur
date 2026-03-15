# claude-worker-rust (Container Image)

Extends `ur-worker:latest` with build dependencies for Rust projects. Cargo and bacon run on the host via hostexec, not inside the container.

- Build context is `containers/claude-worker-rust/` — all files copied into the image must live here
- Image is tagged `ur-worker-rust:latest` by convention
- Inherits everything from `ur-worker` (Claude Code, tmux entrypoint, worker binaries)
- Adds: build-essential, pkg-config, libssl-dev, git for native compilation support
- Rust toolchain (cargo, bacon, etc.) runs on the host via hostexec shims — no mise or local toolchain in the container
- At runtime, workerd creates shims for hostexec commands (git, gh, cargo, bacon, etc.)
