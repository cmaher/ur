ARG BASE_TAG=latest
FROM ur-worker-base:${BASE_TAG}

# Install the codex CLI as root — it's a single native binary at a
# system-wide path, not a per-user install like Claude Code's own installer.
# Root because `install`'s destination-replace step unlinks the existing file
# first, which needs write access to the (root-owned) /usr/local/bin directory
# itself, not just the file.
#
# This is also the *update* layer, which is why the install and its CACHEBUST
# gate are one step rather than claude's install-then-`claude update` pair:
# `codex update` does NOT work here — it refuses to update a binary it didn't
# install itself, failing with "Could not detect the Codex installation
# method" — so re-running this script is the only update mechanism. A separate
# unconditional install layer would just download the same release twice on a
# cold build. Pass --build-arg CACHEBUST=<timestamp> to force a re-install
# (UR_UPDATE_AGENT=codex / UR_FORCE_REBUILD_BASE=1 do this for you via
# scripts/build/image.sh — see cargo make install-update-codex); with the
# default, this layer caches like any other, so codex stays pinned at the baked
# version, and rebuilds that only change the worker binaries below reuse it
# instead of re-downloading.
#
# CACHEBUST must be referenced in the RUN command — BuildKit only invalidates
# cache for layers that actually use the ARG, not merely declare it.
# A failed install fails the build on purpose: silently shipping a stale (or
# missing) codex is worse than a red build, since the cached layer would then
# hide it for weeks.
ARG CACHEBUST=0
COPY vendor/codex/install.sh /tmp/install-codex.sh
RUN echo "CACHEBUST=$CACHEBUST" && bash /tmp/install-codex.sh && codex --version
RUN rm /tmp/install-codex.sh

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

USER worker
WORKDIR /workspace

HEALTHCHECK --interval=1s --timeout=1s --start-period=30s --retries=1 \
    CMD curl -sf http://localhost:9119/healthz || exit 1

ENTRYPOINT ["/entrypoint.sh"]
