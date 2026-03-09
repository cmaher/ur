FROM alpine:3.21

RUN apk add --no-cache \
    docker-cli \
    ca-certificates \
    tini

# Copy the cross-compiled urd binary (staged by the build script)
COPY urd /usr/local/bin/urd
RUN chmod +x /usr/local/bin/urd

# Default gRPC port (ur_config::DEFAULT_DAEMON_PORT)
EXPOSE 42069

ENTRYPOINT ["tini", "--"]
CMD ["urd"]
