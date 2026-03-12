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

# Pre-cache the all-MiniLM-L6-v2 ONNX embedding model so fastembed doesn't
# need to download it at runtime (containers are ephemeral).
# The hf_hub crate (used by fastembed) expects this cache layout:
#   {cache_dir}/models--{org}--{model}/refs/main        -> commit hash
#   {cache_dir}/models--{org}--{model}/snapshots/{hash}/ -> model files
ENV FASTEMBED_CACHE_DIR=/usr/local/share/fastembed
RUN set -eux; \
    MODEL_DIR="$FASTEMBED_CACHE_DIR/models--Qdrant--all-MiniLM-L6-v2-onnx"; \
    COMMIT="5f1b8cd78bc4fb444dd171e59b18f3a3af89a079"; \
    SNAPSHOT_DIR="$MODEL_DIR/snapshots/$COMMIT"; \
    HF_BASE="https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main"; \
    mkdir -p "$MODEL_DIR/refs" "$MODEL_DIR/blobs" "$SNAPSHOT_DIR"; \
    echo -n "$COMMIT" > "$MODEL_DIR/refs/main"; \
    for f in model.onnx tokenizer.json config.json special_tokens_map.json tokenizer_config.json; do \
        wget -q -O "$SNAPSHOT_DIR/$f" "$HF_BASE/$f"; \
    done

# Default gRPC port (ur_config::DEFAULT_DAEMON_PORT)
EXPOSE 42069

ENTRYPOINT ["tini", "--"]
CMD ["ur-server"]
