FROM ur-worker:latest

USER root

# Build dependencies for native compilation support
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    git \
    && rm -rf /var/lib/apt/lists/*

# Custom entrypoint — copied after apt-get so entrypoint changes don't
# invalidate the dependency cache layer.
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

USER worker

ENTRYPOINT ["/entrypoint.sh"]
