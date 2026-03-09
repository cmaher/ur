FROM ur-worker-base:latest

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

# Wrapper that delegates to the real claude with sandboxed permissions
COPY claude-wrapper.sh /usr/local/bin/claude
RUN chmod +x /usr/local/bin/claude

RUN mkdir -p /workspace && chown worker:worker /workspace

# UR_GRPC_HOST and UR_GRPC_PORT are set at runtime by the server when launching the container.
# Worker commands use these to connect back to the host's per-agent gRPC server.

USER worker
WORKDIR /workspace

ENTRYPOINT ["/entrypoint.sh"]
