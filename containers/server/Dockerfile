FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    docker.io \
    ca-certificates \
    git \
    tini \
    netcat-openbsd \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Copy the cross-compiled ur-server binary (staged by the build script)
COPY ur-server /usr/local/bin/ur-server
RUN chmod +x /usr/local/bin/ur-server

# Download ONNX Runtime shared library for fastembed/ort.
# The binary is compiled with ort-load-dynamic, which requires libonnxruntime.so
# to be available via ORT_DYLIB_PATH at runtime.
ARG TARGETARCH
RUN set -eux; \
    ORT_VERSION="1.20.0"; \
    case "$TARGETARCH" in \
        arm64) ORT_ARCH="aarch64" ;; \
        amd64) ORT_ARCH="x64" ;; \
        *) echo "Unsupported arch: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    curl -fSL "https://github.com/microsoft/onnxruntime/releases/download/v${ORT_VERSION}/onnxruntime-linux-${ORT_ARCH}-${ORT_VERSION}.tgz" \
        | tar xz -C /tmp; \
    cp /tmp/onnxruntime-linux-${ORT_ARCH}-${ORT_VERSION}/lib/libonnxruntime.so.${ORT_VERSION} /usr/local/lib/; \
    ln -s libonnxruntime.so.${ORT_VERSION} /usr/local/lib/libonnxruntime.so; \
    ldconfig; \
    rm -rf /tmp/onnxruntime-*

ENV ORT_DYLIB_PATH=/usr/local/lib/libonnxruntime.so

# Default gRPC port (ur_config::DEFAULT_DAEMON_PORT)
EXPOSE 42069

ENTRYPOINT ["tini", "--"]
CMD ["ur-server"]
