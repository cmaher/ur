FROM ur-worker-base:latest

# Install Claude Code CLI as worker user
COPY vendor/claude/install.sh /tmp/install-claude.sh
USER worker
RUN bash /tmp/install-claude.sh
USER root
RUN rm /tmp/install-claude.sh

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Copy pre-built worker command binaries into the image
COPY bin/ur-ping /usr/local/bin/ur-ping
RUN chmod +x /usr/local/bin/ur-ping
COPY bin/workertools /usr/local/bin/workertools
RUN chmod +x /usr/local/bin/workertools

COPY bin/workerd /usr/local/bin/workerd
RUN chmod +x /usr/local/bin/workerd

# Shim directory — workerd writes shims here at startup
RUN mkdir -p /home/worker/.local/bin && chown worker:worker /home/worker/.local/bin
ENV PATH="/usr/local/bin:/home/worker/.local/bin:${PATH}"

RUN mkdir -p /workspace && chown worker:worker /workspace

# Bake in minimal Claude Code config to skip onboarding/login prompts.
# Credentials (.credentials.json) are mounted at runtime; this file is static.
COPY claude.json /home/worker/.claude.json
COPY claude-settings.json /home/worker/.claude/potential-settings.json
RUN mkdir -p /home/worker/.claude/skills
# Pre-create the transcript directory (Claude Code writes ~/.claude/projects/<cwd-slug>/*.jsonl;
# cwd is /workspace → slug is -workspace). The project memory_dir is bind-mounted at
# .../projects/-workspace/memory; if this parent doesn't already exist, the Docker daemon creates
# it as root and the worker user can't write transcripts there (EACCES). Owning it via the chown
# below keeps it writable after the mount.
RUN mkdir -p /home/worker/.claude/projects/-workspace
RUN chown worker:worker /home/worker/.claude.json && chown -R worker:worker /home/worker/.claude

# UR_GRPC_HOST and UR_GRPC_PORT are set at runtime by the server when launching the container.
# Worker commands use these to connect back to the host's per-agent gRPC server.

# Claude Code update. Pass --build-arg CACHEBUST=<timestamp> to bust the cache on
# this layer (cargo make install-update-claude / install-nocache do this for you).
# CACHEBUST must be referenced in the RUN command — BuildKit only invalidates
# cache for layers that actually use the ARG, not merely declare it.
# A failed update fails the build on purpose: silently shipping a stale Claude Code
# is worse than a red build, since the cached layer would then hide it for weeks.
ARG CACHEBUST=0
USER worker
RUN echo "CACHEBUST=$CACHEBUST" && claude update && claude --version
WORKDIR /workspace

HEALTHCHECK --interval=1s --timeout=1s --start-period=30s --retries=1 \
    CMD curl -sf http://localhost:9119/healthz || exit 1

ENTRYPOINT ["/entrypoint.sh"]
