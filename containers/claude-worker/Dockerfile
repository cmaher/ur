FROM ubuntu:24.04

RUN apt-get update && apt-get install -y --no-install-recommends \
    tmux \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# agent_tools binary will be copied in at build time once it exists.
# For now, create placeholder so the image shape is correct.
RUN mkdir -p /usr/local/bin

ENTRYPOINT ["/entrypoint.sh"]
