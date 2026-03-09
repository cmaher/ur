# Ur

Coding LLM coordination framework. Native macOS monolith managing containerized Claude Code workers via gRPC.

## Prerequisites

### Container Runtime

Ur requires Docker with Compose support. On macOS, we recommend [OrbStack](https://orbstack.dev/) as a lightweight, fast alternative to Docker Desktop.

### Development Tools

All project dependencies are managed via [mise](https://mise.jdx.dev/):

```sh
# Install mise (macOS)
brew install mise

# Install all project tools (Rust, protoc, zig, cargo-make, etc.)
mise install
```

## Development

```sh
cargo make ci        # Run all CI checks (fmt, clippy, build, test)
cargo make install   # Build ur CLI + container images, install to ~/bin
```
