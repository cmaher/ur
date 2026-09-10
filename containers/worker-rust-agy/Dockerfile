ARG BASE_TAG=latest
FROM ur-worker-agy:${BASE_TAG}

USER root
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh
USER worker

ENTRYPOINT ["/entrypoint.sh"]
