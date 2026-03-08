FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    bash \
    tmux \
    curl \
    ca-certificates \
    coreutils \
    && rm -rf /var/lib/apt/lists/*

# Install Claude Code CLI (script is cached locally, binary downloaded at build time)
COPY install-claude.sh /tmp/install-claude.sh
RUN bash /tmp/install-claude.sh && rm /tmp/install-claude.sh

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Copy pre-built worker command binaries into the image
COPY ur-ping /usr/local/bin/ur-ping
RUN chmod +x /usr/local/bin/ur-ping
COPY git /usr/local/bin/git
RUN chmod +x /usr/local/bin/git
COPY gh /usr/local/bin/gh
RUN chmod +x /usr/local/bin/gh

COPY tk /usr/local/bin/tk
RUN chmod +x /usr/local/bin/tk

RUN mkdir -p /workspace

# UR_GRPC_HOST and UR_GRPC_PORT are set at runtime by urd when launching the container.
# Worker commands use these to connect back to the host's per-agent gRPC server.

ENTRYPOINT ["/entrypoint.sh"]
