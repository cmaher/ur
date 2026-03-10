FROM alpine:3.21

RUN apk add --no-cache \
    docker-cli \
    ca-certificates \
    git \
    tini \
    netcat-openbsd

# Copy the cross-compiled ur-server binary (staged by the build script)
COPY ur-server /usr/local/bin/ur-server
RUN chmod +x /usr/local/bin/ur-server

# Default gRPC port (ur_config::DEFAULT_DAEMON_PORT)
EXPOSE 42069

ENTRYPOINT ["tini", "--"]
CMD ["ur-server"]
