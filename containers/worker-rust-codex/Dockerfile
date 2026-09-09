ARG BASE_TAG=latest
FROM ur-worker-codex:${BASE_TAG}

USER root

# Custom entrypoint — copied after apt-get so entrypoint changes don't
# invalidate the dependency cache layer.
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER worker

ENTRYPOINT ["/entrypoint.sh"]
