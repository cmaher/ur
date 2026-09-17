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

COPY bin/ur-ping /usr/local/bin/ur-ping
COPY bin/workertools /usr/local/bin/workertools
COPY bin/workerd /usr/local/bin/workerd
RUN chmod +x /usr/local/bin/ur-ping /usr/local/bin/workertools /usr/local/bin/workerd

COPY potential-settings.json /home/worker/.gemini/antigravity-cli/potential-settings.json
COPY onboarding.json /home/worker/.gemini/antigravity-cli/cache/onboarding.json
COPY hooks.json /home/worker/.gemini/config/hooks.json
RUN chown -R worker:worker /home/worker/.gemini

ENV PATH="/usr/local/bin:/home/worker/.local/bin:${PATH}"
ENV AGY_CLI_DISABLE_AUTO_UPDATE=1

USER worker
WORKDIR /workspace

HEALTHCHECK --interval=1s --timeout=1s --start-period=30s --retries=1 \
    CMD curl -sf http://localhost:9119/healthz || exit 1

ENTRYPOINT ["/entrypoint.sh"]
