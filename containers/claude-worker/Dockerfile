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

# agent_tools binary will be copied in at build time once it exists.
# For now, create placeholder so the image shape is correct.
RUN mkdir -p /usr/local/bin

ENTRYPOINT ["/entrypoint.sh"]
