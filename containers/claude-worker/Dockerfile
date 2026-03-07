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

RUN mkdir -p /workspace /var/run/ur

# Worker commands connect to urd via this socket (mounted by the host at run time)
ENV UR_SOCKET=/var/run/ur/ur.sock

ENTRYPOINT ["/entrypoint.sh"]
