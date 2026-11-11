ARG BASE_TAG=latest
FROM ur-worker-base:${BASE_TAG}

RUN mkdir -p /home/worker/.local/bin \
        /home/worker/.gemini/antigravity-cli/cache \
        /home/worker/.gemini/config \
        /workspace && \
    chown -R worker:worker /home/worker/.local /home/worker/.gemini /workspace

# Bumped when the bundled AGY must move past a cached release. Version 1.1.12
# fixed wrapped OSC 8 links and selection inside hyperlink-capable tmux.
ARG CACHEBUST=1
COPY vendor/agy/install.sh /tmp/install-agy.sh
USER worker
RUN echo "CACHEBUST=$CACHEBUST" && \
    bash /tmp/install-agy.sh --dir /home/worker/.local/bin && \
    /home/worker/.local/bin/agy --version
USER root
RUN rm /tmp/install-agy.sh

COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

COPY open-auth-link.sh /usr/local/bin/xdg-open
RUN chmod +x /usr/local/bin/xdg-open

COPY potential-settings.json /home/worker/.gemini/antigravity-cli/potential-settings.json
COPY onboarding.json /home/worker/.gemini/antigravity-cli/cache/onboarding.json
COPY hooks.json /home/worker/.gemini/config/hooks.json
RUN chown -R worker:worker /home/worker/.gemini

ENV PATH="/usr/local/bin:/home/worker/.local/bin:${PATH}"
ENV AGY_CLI_DISABLE_AUTO_UPDATE=1
ENV BROWSER=/usr/local/bin/xdg-open

# Agent-agnostic skills and instructions, from the `shared` named build context
# (containers/worker-base/, passed by scripts/build/image.sh). They sit after
# the agent CLI install/update so editing a skill or instruction file does not
# re-download the agent CLI; only these layers and the binaries below rebuild.
# Order matters: both skill COPYs land in the same directory, and the later one
# wins a name collision. The repo's own skills go second so they override a
# same-named vendored skill, not the other way round.
COPY --from=shared --chown=worker:worker vendor/superpowers/skills/ /home/worker/.agent-shared/potential-skills/
COPY --from=shared --chown=worker:worker potential-skills/ /home/worker/.agent-shared/potential-skills/
COPY --from=shared --chown=worker:worker instructions/ /home/worker/.agent-shared/instructions/
COPY --from=shared --chown=worker:worker shared-instructions/ /home/worker/.agent-shared/shared-instructions/

# Worker binaries change on nearly every commit, so they are the last layer:
# everything above (agent CLI install/update, config, shared assets) stays
# cached across Rust-only changes. --chmod avoids a second full copy from `RUN chmod`.
COPY --chmod=755 bin/ur-workertools /usr/local/bin/ur-workertools
COPY --chmod=755 bin/workerd /usr/local/bin/workerd

USER worker
WORKDIR /workspace

HEALTHCHECK --interval=1s --timeout=1s --start-period=30s --retries=1 \
    CMD curl -sf http://localhost:9119/healthz || exit 1

ENTRYPOINT ["/entrypoint.sh"]
