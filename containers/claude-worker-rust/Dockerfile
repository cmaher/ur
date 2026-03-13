FROM ur-worker:latest

USER root

# Rust-specific entrypoint (adds bacon background watcher)
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Build dependencies for Rust compilation
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

USER worker

# Install mise
COPY install-mise.sh /tmp/install-mise.sh
RUN bash /tmp/install-mise.sh

# Global mise config
COPY mise.toml /home/worker/.config/mise/config.toml

# Ensure ~/.cargo exists as worker before cache mounts create subdirs as root
RUN mkdir -p /home/worker/.cargo

# Auto-trust all config files (build-time and runtime when workspace is mounted).
# Suppress WARN-level messages at runtime — workspace mise.toml references tools
# not installed in the container (e.g., cargo-docs-md) and mise can't reach
# crates.io through the proxy tunnel to resolve them.
ENV MISE_TRUSTED_CONFIG_PATHS=/workspace
ENV MISE_LOG_LEVEL=error
RUN --mount=type=cache,target=/home/worker/.cargo/registry,uid=1000 \
    --mount=type=cache,target=/home/worker/.cargo/git,uid=1000 \
    MISE_YES=1 /home/worker/.local/bin/mise trust /home/worker/.config/mise/config.toml \
    && MISE_YES=1 /home/worker/.local/bin/mise install

# Activate mise in interactive shells so all tools are on PATH.
RUN echo 'eval "$(/home/worker/.local/bin/mise activate bash)"' >> /home/worker/.bashrc

ENTRYPOINT ["/entrypoint.sh"]
