# claude-worker-rust (Container Image)

Extends `ur-worker:latest` with a full Rust development toolchain installed via mise.

- Build context is `containers/claude-worker-rust/` — all files copied into the image must live here
- Image is tagged `ur-worker-rust:latest` by convention
- Inherits everything from `ur-worker` (Claude Code, tmux entrypoint, worker binaries)
- Adds: build-essential, pkg-config, libssl-dev, git for native compilation
- mise installs: rust (stable), zig, protoc, cargo-make, cargo-zigbuild, cargo-audit, bacon
- mise activates in `.bashrc` so all tools are on PATH in tmux/interactive sessions
- At runtime, ur-workerd creates shims that shadow system git/gh; no proxy juggling needed at build time
