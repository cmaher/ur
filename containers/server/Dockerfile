FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    docker.io \
    ca-certificates \
    git \
    tini \
    netcat-openbsd \
    && rm -rf /var/lib/apt/lists/*

# Copy the cross-compiled ur-server binary (staged by the build script)
COPY ur-server /usr/local/bin/ur-server
RUN chmod +x /usr/local/bin/ur-server

# Default gRPC port (ur_config::DEFAULT_SERVER_PORT)
EXPOSE 12321

ENTRYPOINT ["tini", "--"]
CMD ["ur-server"]
