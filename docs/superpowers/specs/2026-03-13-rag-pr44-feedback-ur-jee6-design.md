# RAG PR #44 Feedback: Model Caching, Proto Defaults, Configurable Model (ur-jee6)

Addresses three review comments on PR #44.

## 1. Host-side model caching (essential)

**Problem**: Downloading the embedding model during `docker build` kills OrbStack.

**Solution**: Download the model once on the host, cache it in `$UR_CONFIG/fastembed/`, and mount that directory into the server container.

### Cache layout

fastembed uses hf_hub's cache convention:

```
~/.ur/fastembed/
  models--Qdrant--all-MiniLM-L6-v2-onnx/
    refs/main                              → commit hash string
    snapshots/{hash}/                      → model.onnx, tokenizer.json, config.json, ...
```

### Download triggers

1. **`cargo make install`** — `scripts/build/install.sh` checks if the model directory exists under `${UR_CONFIG:-~/.ur}/fastembed/`. If missing, curls the files from HuggingFace. Same files as the old Dockerfile wget, just targeting the host path with curl.

2. **`ur rag model download`** — explicit CLI command. Same curl-based download logic. Useful for re-downloading or after changing the configured model.

### Dockerfile changes

Remove lines 14-29 (the `FASTEMBED_CACHE_DIR` env and the `RUN` layer that wgets model files). The model comes from the mount at runtime.

### Docker Compose changes

Add to `ur-server` in `docker-compose.yml`:

```yaml
volumes:
  - ${UR_CONFIG:-~/.ur}/fastembed:/fastembed:ro
environment:
  - FASTEMBED_CACHE_DIR=/fastembed
```

Read-only mount (`:ro`) — the server only reads the model at runtime, never writes.

### Server startup

The server sets `FASTEMBED_CACHE_DIR` via the compose env var. fastembed finds the cached model there. If the model is missing, the server panics with a clear error: "embedding model not found — run `ur rag model download`".

## 2. Proto `LANGUAGE_UNSPECIFIED` default

**Problem**: `LANGUAGE_RUST = 0` is the proto default, violating the proto3 convention that 0 should be an unspecified/sentinel value.

**Solution**:

```protobuf
enum Language {
  LANGUAGE_UNSPECIFIED = 0;
  LANGUAGE_RUST = 1;
}
```

The CLI still requires an explicit language argument. `LANGUAGE_UNSPECIFIED` exists for proto default safety. The server rejects `LANGUAGE_UNSPECIFIED` with an `InvalidArgument` gRPC status.

Future: project-level default language in `ur.toml` project config, injected into workers via env var. Not in scope for this change.

Note: this is a wire-incompatible renumber, but there is no data compatibility issue — Qdrant payloads store language as a string (`"rust"`), not the enum integer. Existing collections do not need re-indexing.

## 3. Configurable embedding model

**Problem**: The embedding model is hardcoded to `AllMiniLML6V2`.

**Solution**: Add `embedding_model` to the `[rag]` section of `ur.toml`:

```toml
[rag]
qdrant_hostname = "ur-qdrant"
embedding_model = "all-MiniLM-L6-v2"
```

### Config changes

`RawRagConfig` gains `embedding_model: Option<String>`. `RagConfig` gains `embedding_model: String` (default: `"all-MiniLM-L6-v2"`).

### Model name mapping

A `ModelInfo` struct in the `rag` crate maps the config string (kebab-case, e.g. `"all-MiniLM-L6-v2"`) to:
- `fastembed::EmbeddingModel` variant (for server init)
- HuggingFace org/repo, commit hash, and file list (for curl download)
- Vector dimension (e.g. 384 for MiniLM) — used by `RagManager::recreate_collection` instead of the current hardcoded `VECTOR_SIZE`

This is the single source of truth for model metadata. Both the Rust CLI (`ur rag model download`) and the install script consume it. The install script cannot call the ur binary, so it hardcodes the default model's download info (commit hash, files). This is acceptable duplication — the install script only downloads the default model. Users with custom models use `ur rag model download` after install.

Only `all-MiniLM-L6-v2` is supported initially. Unknown model names produce a clear error listing supported models.

### Consumers

- **Server** (`crates/server/src/main.rs`): reads `cfg.rag.embedding_model`, maps to `fastembed::EmbeddingModel`, passes to `TextEmbedding::try_new`.
- **Install script** (`scripts/build/install.sh`): always downloads the default model (`all-MiniLM-L6-v2`). Does not parse `ur.toml` — keeps the script simple with no TOML parsing dependency.
- **`ur rag model download`**: reads `ur.toml` config, downloads the configured model. This is how users get non-default models.

## Files changed

**Modified:**
- `proto/rag.proto` — add `LANGUAGE_UNSPECIFIED = 0`, shift `LANGUAGE_RUST` to 1
- `containers/server/Dockerfile` — remove model download layer
- `containers/docker-compose.yml` — add fastembed volume mount + env var to ur-server
- `scripts/build/install.sh` — add model download step (curl)
- `crates/ur_config/src/lib.rs` — add `embedding_model` to `RagConfig`
- `crates/server/src/main.rs` — use config for model selection, improve error message
- `crates/server/src/rag.rs` — reject `LANGUAGE_UNSPECIFIED` with `InvalidArgument`
- `crates/rag/` — add model name → fastembed enum mapping, add model name → HF download info mapping
- `crates/ur/src/rag.rs` — add `ur rag model download` subcommand
- `crates/ur/src/main.rs` — wire up `model download` subcommand
- `crates/rag/src/lib.rs` — export `ModelInfo` and mapping functions

## Tests

- `crates/rag/`: unit tests for model name mapping (valid names, invalid names, returned metadata)
- `crates/ur_config/`: config parsing tests for `embedding_model` field (absent → default, custom value, empty `[rag]` section)
- `crates/server/src/rag.rs`: `LANGUAGE_UNSPECIFIED` rejection returns `InvalidArgument`
