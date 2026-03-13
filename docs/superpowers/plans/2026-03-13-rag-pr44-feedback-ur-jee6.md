# RAG PR #44 Feedback: Model Caching, Proto Defaults, Configurable Model (ur-jee6)

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Address three PR #44 review comments: host-side embedding model caching, proto `LANGUAGE_UNSPECIFIED` default, and configurable embedding model via `ur.toml`.

**Architecture:** Move the embedding model download from the Docker build layer to the host filesystem (`~/.ur/fastembed/`), mounted read-only into the server container. Add a `ModelInfo` struct in the `rag` crate as the single source of truth for model metadata. Shift the proto `Language` enum to start at 1 with an `UNSPECIFIED` sentinel at 0.

**Tech Stack:** Rust, tonic/prost (protobuf), fastembed, Docker, shell (curl)

---

## Chunk 1: Proto LANGUAGE_UNSPECIFIED + Server Rejection

### Task 1: Add LANGUAGE_UNSPECIFIED to proto enum

**Files:**
- Modify: `proto/rag.proto:5-7`

- [ ] **Step 1: Update the proto enum**

```protobuf
enum Language {
  LANGUAGE_UNSPECIFIED = 0;
  LANGUAGE_RUST = 1;
}
```

- [ ] **Step 2: Rebuild proto codegen**

Run: `cargo build -p ur_rpc`
Expected: PASS — the generated code now has `Language::Unspecified` and `Language::Rust`.

- [ ] **Step 3: Fix compile errors from enum shift**

The `language_str` match in `crates/server/src/rag.rs:87-91` and `parse_language` in `crates/ur/src/rag.rs:60-65` both need updating for the new variant names and the new `Unspecified` variant.

In `crates/server/src/rag.rs`, update `language_str`:

```rust
fn language_str(lang: Language) -> Result<&'static str, Status> {
    match lang {
        Language::Unspecified => Err(Status::invalid_argument(
            "language is required — specify a language (e.g. --language rust)",
        )),
        Language::Rust => Ok("rust"),
    }
}
```

In `crates/ur/src/rag.rs`, update `parse_language` — no change needed for the match body (it matches on the string `"rust"`), but the variant name changes from `Language::Rust` to `Language::Rust` (stays the same since prost generates CamelCase from `LANGUAGE_RUST`). Verify by checking generated code — prost strips the enum prefix, so `LANGUAGE_RUST` becomes `Rust` and `LANGUAGE_UNSPECIFIED` becomes `Unspecified`. No change needed in `parse_language`.

- [ ] **Step 4: Update rag_index and rag_search to propagate the error**

In `crates/server/src/rag.rs`, both `rag_index` and `rag_search` currently call `language_str(req.language())` without `?`. Since `language_str` now returns `Result`, add `?`:

In `rag_index` (line 27):
```rust
let language = language_str(req.language())?;
```

In `rag_search` (line 57):
```rust
let language = language_str(req.language())?;
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build -p ur_rpc -p ur-server -p ur`
Expected: PASS

- [ ] **Step 6: Commit**

```
feat(proto): add LANGUAGE_UNSPECIFIED sentinel to rag.proto (ur-jee6)

Shifts LANGUAGE_RUST from 0 to 1, adding LANGUAGE_UNSPECIFIED = 0
as the proto3-conventional default. Server rejects Unspecified with
InvalidArgument.
```

### Task 2: Add server-side test for LANGUAGE_UNSPECIFIED rejection

**Files:**
- Modify: `crates/server/src/rag.rs`

- [ ] **Step 1: Write the test**

Add a `#[cfg(test)]` module at the bottom of `crates/server/src/rag.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_str_rejects_unspecified() {
        let result = language_str(Language::Unspecified);
        assert!(result.is_err());
        let status = result.unwrap_err();
        assert_eq!(status.code(), tonic::Code::InvalidArgument);
    }

    #[test]
    fn language_str_accepts_rust() {
        let result = language_str(Language::Rust);
        assert_eq!(result.unwrap(), "rust");
    }
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test -p ur-server`
Expected: PASS — both tests should pass.

- [ ] **Step 3: Commit**

```
test(server): add language_str rejection tests (ur-jee6)
```

## Chunk 2: ModelInfo + Configurable Embedding Model

### Task 3: Create ModelInfo struct in rag crate

**Files:**
- Create: `crates/rag/src/model.rs`
- Modify: `crates/rag/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rag/src/model.rs` with the test module first:

```rust
/// Metadata for a supported embedding model.
///
/// Single source of truth for model name → fastembed enum, HuggingFace download
/// info, and vector dimensions.
pub struct ModelInfo {
    /// fastembed enum variant for initializing the model.
    pub fastembed_model: fastembed::EmbeddingModel,
    /// HuggingFace org (e.g. "Qdrant").
    pub hf_org: &'static str,
    /// HuggingFace repo name (e.g. "all-MiniLM-L6-v2-onnx").
    pub hf_repo: &'static str,
    /// Git commit hash for the snapshot.
    pub hf_commit: &'static str,
    /// Files to download from HuggingFace.
    pub hf_files: &'static [&'static str],
    /// Vector dimensionality (e.g. 384 for MiniLM).
    pub vector_size: u64,
}

/// Look up model info by config name (kebab-case, e.g. "all-MiniLM-L6-v2").
///
/// Returns `None` for unknown model names.
pub fn model_info(name: &str) -> Option<&'static ModelInfo> {
    SUPPORTED_MODELS.iter().find(|(n, _)| *n == name).map(|(_, info)| info)
}

/// List all supported model names.
pub fn supported_model_names() -> Vec<&'static str> {
    SUPPORTED_MODELS.iter().map(|(n, _)| *n).collect()
}

/// Default embedding model name.
pub const DEFAULT_EMBEDDING_MODEL: &str = "all-MiniLM-L6-v2";

const MINI_LM_FILES: &[&str] = &[
    "model.onnx",
    "tokenizer.json",
    "config.json",
    "special_tokens_map.json",
    "tokenizer_config.json",
];

static SUPPORTED_MODELS: &[(&str, ModelInfo)] = &[
    (
        "all-MiniLM-L6-v2",
        ModelInfo {
            fastembed_model: fastembed::EmbeddingModel::AllMiniLML6V2,
            hf_org: "Qdrant",
            hf_repo: "all-MiniLM-L6-v2-onnx",
            hf_commit: "5f1b8cd78bc4fb444dd171e59b18f3a3af89a079",
            hf_files: MINI_LM_FILES,
            vector_size: 384,
        },
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_valid_model() {
        let info = model_info("all-MiniLM-L6-v2").unwrap();
        assert_eq!(info.vector_size, 384);
        assert_eq!(info.hf_org, "Qdrant");
        assert_eq!(info.hf_repo, "all-MiniLM-L6-v2-onnx");
        assert!(!info.hf_files.is_empty());
    }

    #[test]
    fn lookup_unknown_model_returns_none() {
        assert!(model_info("nonexistent-model").is_none());
    }

    #[test]
    fn default_model_is_valid() {
        assert!(model_info(DEFAULT_EMBEDDING_MODEL).is_some());
    }

    #[test]
    fn supported_names_includes_default() {
        let names = supported_model_names();
        assert!(names.contains(&DEFAULT_EMBEDDING_MODEL));
    }
}
```

- [ ] **Step 2: Export from lib.rs**

In `crates/rag/src/lib.rs`, add:

```rust
pub mod model;
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p rag`
Expected: PASS — all 4 new tests pass plus existing tests.

- [ ] **Step 4: Commit**

```
feat(rag): add ModelInfo struct for embedding model metadata (ur-jee6)

Single source of truth mapping config model names to fastembed enum
variants, HuggingFace download info, and vector dimensions.
```

### Task 4: Use ModelInfo vector_size in RagManager

**Files:**
- Modify: `crates/rag/src/manager.rs:16`

- [ ] **Step 1: Replace hardcoded VECTOR_SIZE**

In `crates/rag/src/manager.rs`, change the `RagManager` struct to hold vector_size and use it in `recreate_collection`.

Remove `const VECTOR_SIZE: u64 = 384;` (line 16).

Add `vector_size: u64` field to `RagManager`:

```rust
pub struct RagManager {
    qdrant: Arc<qdrant_client::Qdrant>,
    embedding_model: Arc<TextEmbedding>,
    vector_size: u64,
}
```

Update the constructor:

```rust
pub fn new(qdrant: Arc<qdrant_client::Qdrant>, embedding_model: Arc<TextEmbedding>, vector_size: u64) -> Self {
    Self {
        qdrant,
        embedding_model,
        vector_size,
    }
}
```

Update `recreate_collection` to use `self.vector_size` instead of `VECTOR_SIZE`:

```rust
.vectors_config(VectorParamsBuilder::new(self.vector_size, Distance::Cosine)),
```

- [ ] **Step 2: Fix compile errors in server**

In `crates/server/src/main.rs:144`, update the `RagManager::new` call to pass `vector_size`. For now use 384 directly — the next task wires up the config:

```rust
let rag_manager = rag::RagManager::new(qdrant, embedding_model, 384);
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build -p rag -p ur-server`
Expected: PASS

- [ ] **Step 4: Commit**

```
refactor(rag): accept vector_size in RagManager constructor (ur-jee6)

Replaces hardcoded VECTOR_SIZE constant with a constructor parameter,
preparing for configurable embedding models with different dimensions.
```

### Task 5: Add embedding_model to ur.toml config

**Files:**
- Modify: `crates/ur_config/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add these tests to the existing `#[cfg(test)]` module in `crates/ur_config/src/lib.rs`:

```rust
#[test]
fn rag_embedding_model_defaults_when_absent() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("ur.toml"), "").unwrap();
    let cfg = Config::load_from(tmp.path()).unwrap();
    assert_eq!(cfg.rag.embedding_model, "all-MiniLM-L6-v2");
}

#[test]
fn rag_reads_custom_embedding_model() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(
        tmp.path().join("ur.toml"),
        "[rag]\nembedding_model = \"custom-model\"\n",
    )
    .unwrap();
    let cfg = Config::load_from(tmp.path()).unwrap();
    assert_eq!(cfg.rag.embedding_model, "custom-model");
}

#[test]
fn rag_embedding_model_defaults_when_rag_section_empty() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("ur.toml"), "[rag]\n").unwrap();
    let cfg = Config::load_from(tmp.path()).unwrap();
    assert_eq!(cfg.rag.embedding_model, "all-MiniLM-L6-v2");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p ur_config`
Expected: FAIL — `embedding_model` field doesn't exist yet.

- [ ] **Step 3: Add the field to config structs**

In `crates/ur_config/src/lib.rs`:

Add the default constant near the other RAG defaults (after line 168):
```rust
/// Default embedding model name for RAG.
pub const DEFAULT_EMBEDDING_MODEL: &str = "all-MiniLM-L6-v2";
```

Add `embedding_model` to `RawRagConfig` (line 227):
```rust
struct RawRagConfig {
    qdrant_hostname: Option<String>,
    embedding_model: Option<String>,
}
```

Add `embedding_model` to `RagConfig` (line 258):
```rust
pub struct RagConfig {
    pub qdrant_hostname: String,
    pub embedding_model: String,
}
```

Update the `rag` resolution in `Config::load_from` (lines 389-398):
```rust
let rag = match raw.rag {
    Some(r) => RagConfig {
        qdrant_hostname: r
            .qdrant_hostname
            .unwrap_or_else(|| DEFAULT_QDRANT_HOSTNAME.to_string()),
        embedding_model: r
            .embedding_model
            .unwrap_or_else(|| DEFAULT_EMBEDDING_MODEL.to_string()),
    },
    None => RagConfig {
        qdrant_hostname: DEFAULT_QDRANT_HOSTNAME.to_string(),
        embedding_model: DEFAULT_EMBEDDING_MODEL.to_string(),
    },
};
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p ur_config`
Expected: PASS — all tests pass including the 3 new ones.

- [ ] **Step 5: Commit**

```
feat(config): add embedding_model to [rag] config section (ur-jee6)

Defaults to "all-MiniLM-L6-v2". Users can override in ur.toml to use
a different supported embedding model.
```

### Task 6: Wire configurable model into server startup

**Files:**
- Modify: `crates/server/src/main.rs:119-150`

- [ ] **Step 1: Use config model name to look up ModelInfo**

Replace the server's RAG initialization block (`crates/server/src/main.rs:119-150`) with:

```rust
#[cfg(feature = "rag")]
let rag_handler = {
    use std::sync::Arc;

    let model = rag::model::model_info(&cfg.rag.embedding_model).unwrap_or_else(|| {
        let supported = rag::model::supported_model_names().join(", ");
        panic!(
            "unknown embedding model '{}' — supported models: {supported}",
            cfg.rag.embedding_model,
        );
    });

    let qdrant_url = format!(
        "http://{}:{}",
        cfg.rag.qdrant_hostname,
        ur_config::DEFAULT_QDRANT_PORT,
    );
    info!(qdrant_url = %qdrant_url, "connecting to Qdrant");

    let qdrant = Arc::new(
        qdrant_client::Qdrant::from_url(&qdrant_url)
            .build()
            .expect("failed to create Qdrant client"),
    );

    let embedding_model = Arc::new(
        fastembed::TextEmbedding::try_new(
            fastembed::InitOptions::new(model.fastembed_model.clone())
                .with_show_download_progress(false),
        )
        .expect("failed to load embedding model — run `ur rag model download`"),
    );

    let rag_manager = rag::RagManager::new(qdrant, embedding_model, model.vector_size);

    Some(ur_server::rag::RagServiceHandler {
        rag_manager,
        config_dir: cfg.config_dir.clone(),
    })
};
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build -p ur-server`
Expected: PASS

- [ ] **Step 3: Commit**

```
feat(server): use configured embedding model from ur.toml (ur-jee6)

Server looks up the model name in ModelInfo, gets the fastembed enum
variant and vector dimensions from there instead of hardcoding.
```

## Chunk 3: Host-Side Model Caching

### Task 7: Add `ur rag model download` CLI command

**Files:**
- Modify: `crates/ur/src/main.rs:120-141`
- Modify: `crates/ur/src/rag.rs`

- [ ] **Step 1: Add the Model subcommand to RagCommands**

In `crates/ur/src/main.rs`, update `RagCommands`:

```rust
#[derive(Subcommand)]
enum RagCommands {
    /// Generate Rust documentation for RAG indexing
    Docs,
    /// Index generated docs into the vector store
    Index {
        /// Language to index (default: rust)
        #[arg(long, default_value = "rust")]
        language: String,
    },
    /// Search indexed documentation
    Search {
        /// Search query
        query: String,
        /// Language to search (default: rust)
        #[arg(long, default_value = "rust")]
        language: String,
        /// Number of results to return (default: 5)
        #[arg(long, default_value = "5")]
        top_k: u32,
    },
    /// Manage embedding models
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
}

#[derive(Subcommand)]
enum ModelCommands {
    /// Download the configured embedding model to the local cache
    Download,
}
```

- [ ] **Step 2: Wire up the Model dispatch**

In `crates/ur/src/main.rs`, update the `Commands::Rag` match arm (around line 603):

```rust
Commands::Rag { command } => match command {
    RagCommands::Docs => rag::generate_docs(&config.config_dir)?,
    RagCommands::Index { language } => rag::index(port, &language).await?,
    RagCommands::Search {
        query,
        language,
        top_k,
    } => rag::search(port, &query, &language, top_k).await?,
    RagCommands::Model { command } => match command {
        ModelCommands::Download => rag::download_model(&config)?,
    },
},
```

- [ ] **Step 3: Implement the download function**

Add `download_model` to `crates/ur/src/rag.rs`:

```rust
use rag::model;

/// Download the configured embedding model to the local fastembed cache.
///
/// Uses curl to fetch model files from HuggingFace, matching the hf_hub
/// cache layout that fastembed expects.
pub fn download_model(config: &ur_config::Config) -> Result<()> {
    let model_name = &config.rag.embedding_model;
    let info = model::model_info(model_name).ok_or_else(|| {
        let supported = model::supported_model_names().join(", ");
        anyhow::anyhow!(
            "unknown embedding model '{model_name}' — supported models: {supported}"
        )
    })?;

    let cache_dir = config.config_dir.join("fastembed");
    let model_dir = cache_dir.join(format!("models--{}--{}", info.hf_org, info.hf_repo));
    let snapshot_dir = model_dir.join("snapshots").join(info.hf_commit);

    // Check if already downloaded
    if snapshot_dir.exists() {
        let all_present = info.hf_files.iter().all(|f| snapshot_dir.join(f).exists());
        if all_present {
            println!("Model '{model_name}' already downloaded at {}", cache_dir.display());
            return Ok(());
        }
    }

    println!("Downloading model '{model_name}' to {}...", cache_dir.display());

    std::fs::create_dir_all(model_dir.join("refs"))
        .context("failed to create model refs directory")?;
    std::fs::create_dir_all(model_dir.join("blobs"))
        .context("failed to create model blobs directory")?;
    std::fs::create_dir_all(&snapshot_dir)
        .context("failed to create model snapshot directory")?;

    // Write the commit hash to refs/main
    std::fs::write(model_dir.join("refs").join("main"), info.hf_commit)
        .context("failed to write refs/main")?;

    let hf_base = format!(
        "https://huggingface.co/{}/{}/resolve/main",
        info.hf_org, info.hf_repo
    );

    for file in info.hf_files {
        let url = format!("{hf_base}/{file}");
        let dest = snapshot_dir.join(file);
        println!("  Downloading {file}...");

        let status = Command::new("curl")
            .args(["-fSL", "-o"])
            .arg(&dest)
            .arg(&url)
            .status()
            .with_context(|| format!("failed to run curl for {file}"))?;

        if !status.success() {
            bail!("curl failed to download {url} (exit code: {:?})", status.code());
        }
    }

    println!("Done. Model '{}' cached at {}", model_name, cache_dir.display());
    Ok(())
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p ur`
Expected: PASS

- [ ] **Step 5: Commit**

```
feat(cli): add `ur rag model download` command (ur-jee6)

Downloads the configured embedding model from HuggingFace to
~/.ur/fastembed/ using curl, matching the hf_hub cache layout.
```

### Task 8: Remove model download from Dockerfile

**Files:**
- Modify: `containers/server/Dockerfile:14-29`

- [ ] **Step 1: Remove the model download layer**

Delete lines 14-29 from `containers/server/Dockerfile` (the `# Pre-cache...` comment, `ENV FASTEMBED_CACHE_DIR`, and the `RUN` layer). The file should become:

```dockerfile
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
```

- [ ] **Step 2: Commit**

```
refactor(container): remove embedding model download from Dockerfile (ur-jee6)

Model is now cached on the host (~/.ur/fastembed/) and mounted into
the container at runtime via docker-compose.
```

### Task 9: Add fastembed volume mount to docker-compose

**Files:**
- Modify: `containers/docker-compose.yml:34-56`

- [ ] **Step 1: Add the volume mount and env var**

In `containers/docker-compose.yml`, add the fastembed mount to the `ur-server` volumes list (after the workspace volume, line 43):

```yaml
      - ${UR_CONFIG:-~/.ur}/fastembed:/fastembed:ro
```

Add `FASTEMBED_CACHE_DIR` to the environment list (after the `UR_HOST_WORKSPACE` line, line 51):

```yaml
      - FASTEMBED_CACHE_DIR=/fastembed
```

The ur-server volumes section should look like:

```yaml
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock
      - ${UR_CONFIG:-~/.ur}:/config
      - ${UR_WORKSPACE:-~/.ur/workspace}:/workspace
      - ${UR_CONFIG:-~/.ur}/fastembed:/fastembed:ro
```

And the environment section gains:

```yaml
      - FASTEMBED_CACHE_DIR=/fastembed
```

- [ ] **Step 2: Commit**

```
feat(compose): mount host fastembed cache into server container (ur-jee6)

Read-only mount of ~/.ur/fastembed/ at /fastembed inside the server
container. FASTEMBED_CACHE_DIR env var tells fastembed where to find it.
```

### Task 10: Add model download to install script

**Files:**
- Modify: `scripts/build/install.sh`

- [ ] **Step 1: Add curl-based model download**

Append the following to `scripts/build/install.sh`, after the "Installed ur and ur-hostd" echo (line 33):

```bash

# Download the default embedding model for RAG if not already cached.
# This matches the hf_hub cache layout fastembed expects.
FASTEMBED_DIR="${UR_CONFIG:-$HOME/.ur}/fastembed"
MODEL_DIR="$FASTEMBED_DIR/models--Qdrant--all-MiniLM-L6-v2-onnx"
COMMIT="5f1b8cd78bc4fb444dd171e59b18f3a3af89a079"
SNAPSHOT_DIR="$MODEL_DIR/snapshots/$COMMIT"

if [ -d "$SNAPSHOT_DIR" ] && [ -f "$SNAPSHOT_DIR/model.onnx" ]; then
    echo "Embedding model already cached at $FASTEMBED_DIR"
else
    echo "Downloading embedding model (all-MiniLM-L6-v2)..."
    mkdir -p "$MODEL_DIR/refs" "$MODEL_DIR/blobs" "$SNAPSHOT_DIR"
    echo -n "$COMMIT" > "$MODEL_DIR/refs/main"
    HF_BASE="https://huggingface.co/Qdrant/all-MiniLM-L6-v2-onnx/resolve/main"
    for f in model.onnx tokenizer.json config.json special_tokens_map.json tokenizer_config.json; do
        curl -fSL -o "$SNAPSHOT_DIR/$f" "$HF_BASE/$f"
    done
    echo "Embedding model cached at $FASTEMBED_DIR"
fi
```

- [ ] **Step 2: Commit**

```
feat(install): download default embedding model during install (ur-jee6)

Caches all-MiniLM-L6-v2 in ~/.ur/fastembed/ using curl. Skips if
already present. Users with custom models use `ur rag model download`.
```

## Chunk 4: Verification

### Task 11: Final verification

- [ ] **Step 1: Run all tests**

Run: `cargo test --workspace`
Expected: PASS — all existing and new tests pass.

- [ ] **Step 2: Run clippy**

Run: `cargo make clippy`
Expected: PASS — no warnings.

- [ ] **Step 3: Run format check**

Run: `cargo make fmt-fix`
Expected: No changes needed (or apply and commit).

- [ ] **Step 4: Read `.bacon-locations` for any diagnostics**

Check for lingering errors or warnings from the background checker.
