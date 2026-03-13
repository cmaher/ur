FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    docker.io \
    ca-certificates \
    git \
    tini \
    netcat-openbsd \
    && rm -rf /var/lib/apt/lists/*

# ONNX Runtime shared library for fastembed/ort (ort-load-dynamic).
# Staged into build context by scripts/build/image.sh from $UR_CONFIG/onnx/ cache.
COPY libonnxruntime.so /usr/local/lib/libonnxruntime.so
RUN ldconfig

ENV ORT_DYLIB_PATH=/usr/local/lib/libonnxruntime.so

# Copy the cross-compiled ur-server binary (staged by the build script)
COPY ur-server /usr/local/bin/ur-server
RUN chmod +x /usr/local/bin/ur-server

# Default gRPC port (ur_config::DEFAULT_DAEMON_PORT)
EXPOSE 42069

ENTRYPOINT ["tini", "--"]
CMD ["ur-server"]
