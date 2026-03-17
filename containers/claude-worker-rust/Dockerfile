FROM ur-worker:latest

USER root

# Custom entrypoint — copied after apt-get so entrypoint changes don't
# invalidate the dependency cache layer.
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

COPY skill-hooks/ /home/worker/.claude/skill-hooks/

USER worker

ENTRYPOINT ["/entrypoint.sh"]
