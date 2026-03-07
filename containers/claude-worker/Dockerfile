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

# Copy the pre-built agent_tools binary into the image
COPY agent_tools /usr/local/bin/agent_tools
RUN chmod +x /usr/local/bin/agent_tools

RUN mkdir -p /workspace

# agent_tools connects to urd via this socket (mounted by the host at run time)
ENV UR_SOCKET=/var/run/ur/ur.sock

ENTRYPOINT ["/entrypoint.sh"]
