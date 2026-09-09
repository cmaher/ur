ARG BASE_TAG=latest
FROM ur-worker-base:${BASE_TAG}

# Install the codex CLI as root — it's a single native binary at a
# system-wide path, not a per-user install like Claude Code's own installer.
COPY vendor/codex/install.sh /tmp/install-codex.sh
RUN bash /tmp/install-codex.sh
RUN rm /tmp/install-codex.sh
# `codex update` (below) runs as the worker user and needs to overwrite the
# binary this just installed as root.
RUN chown worker:worker /usr/local/bin/codex

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

# Bake in codex config. The on-disk filename is potential-settings.json, not
# .toml — InitSettingsManager's source filename is hardcoded to
# "potential-settings.json" and does a byte-verbatim copy to
# ~/.codex/config.toml regardless of content format. See CLAUDE.md.
RUN mkdir -p /home/worker/.codex/skills
COPY codex-config.toml /home/worker/.codex/potential-settings.json
RUN chown -R worker:worker /home/worker/.codex

# Managed hooks: baked as root at a path only root can write, so hooks fire
# regardless of the per-user/per-repo trust gate. See CLAUDE.md.
RUN mkdir -p /etc/codex
COPY managed_config.toml /etc/codex/managed_config.toml

# UR_GRPC_HOST and UR_GRPC_PORT are set at runtime by the server when launching the container.

# Codex CLI update. Pass --build-arg CACHEBUST=<timestamp> to bust the cache
# on this layer (UR_UPDATE_AGENT=codex / UR_FORCE_REBUILD_BASE=1 do this for
# you via scripts/build/image.sh — see cargo make install-update-claude for
# the equivalent claude-only task).
# CACHEBUST must be referenced in the RUN command — BuildKit only invalidates
# cache for layers that actually use the ARG, not merely declare it.
# A failed update fails the build on purpose: silently shipping a stale codex
# is worse than a red build, since the cached layer would then hide it for weeks.
ARG CACHEBUST=0
USER worker
RUN echo "CACHEBUST=$CACHEBUST" && codex update && codex --version
WORKDIR /workspace

HEALTHCHECK --interval=1s --timeout=1s --start-period=30s --retries=1 \
    CMD curl -sf http://localhost:9119/healthz || exit 1

ENTRYPOINT ["/entrypoint.sh"]
