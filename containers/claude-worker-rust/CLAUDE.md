# claude-worker-rust (Container Image)

Extends `ur-worker:latest` with a full Rust development toolchain installed via mise.

- Build context is `containers/claude-worker-rust/` — all files copied into the image must live here
- Image is tagged `ur-worker-rust:latest` by convention
- Inherits everything from `ur-worker` (Claude Code, tmux entrypoint, git/gh proxies, worker binaries)
- Adds: build-essential, pkg-config, libssl-dev for native compilation
- mise installs: rust (stable), zig, protoc, cargo-make, cargo-zigbuild, cargo-audit, bacon
- mise activates in `.bashrc` so all tools are on PATH in tmux/interactive sessions
- During build, the git proxy is temporarily moved aside since there's no gRPC server; real git from apt is used for tool installation, then the proxy is restored for runtime
