FROM alpine:3.21

RUN apk add --no-cache \
    bash \
    tmux \
    curl \
    ca-certificates \
    coreutils

# Install Claude Code CLI (script is cached locally, binary downloaded at build time)
COPY install-claude.sh /tmp/install-claude.sh
RUN bash /tmp/install-claude.sh && rm /tmp/install-claude.sh

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Copy the pre-built agent_tools binary into the image
COPY agent_tools /usr/local/bin/agent_tools
RUN chmod +x /usr/local/bin/agent_tools

ENTRYPOINT ["/entrypoint.sh"]
