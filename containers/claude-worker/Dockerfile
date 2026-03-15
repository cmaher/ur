FROM ur-worker-base:latest

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Copy pre-built worker command binaries into the image
COPY ur-ping /usr/local/bin/ur-ping
RUN chmod +x /usr/local/bin/ur-ping
COPY workertools /usr/local/bin/workertools
RUN chmod +x /usr/local/bin/workertools

COPY workerd /usr/local/bin/workerd
RUN chmod +x /usr/local/bin/workerd

# Shim directory — workerd writes shims here at startup
RUN mkdir -p /home/worker/.local/bin && chown worker:worker /home/worker/.local/bin
ENV PATH="/usr/local/bin:/home/worker/.local/bin:${PATH}"

COPY tk /usr/local/bin/tk
RUN chmod +x /usr/local/bin/tk

RUN mkdir -p /workspace && chown worker:worker /workspace

# Bake in minimal Claude Code config to skip onboarding/login prompts.
# Credentials (.credentials.json) are mounted at runtime; this file is static.
COPY claude.json /home/worker/.claude.json
COPY claude-settings.json /home/worker/.claude/settings.json
COPY vendor/superpowers/skills/ /home/worker/.claude/potential-skills/
COPY all-skills/ /home/worker/.claude/potential-skills/
RUN mkdir -p /home/worker/.claude/skills
RUN chown worker:worker /home/worker/.claude.json && chown -R worker:worker /home/worker/.claude

# UR_GRPC_HOST and UR_GRPC_PORT are set at runtime by the server when launching the container.
# Worker commands use these to connect back to the host's per-agent gRPC server.

USER worker
WORKDIR /workspace

ENTRYPOINT ["/entrypoint.sh"]
