use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};
use ur_rpc::proto::rag::rag_service_client::RagServiceClient;
use ur_rpc::proto::rag::{Language, RagIndexRequest, RagSearchRequest};

/// Generate Rust documentation into the RAG docs directory.
///
/// Shells out to `cargo-docs-md` to collect sources and produce markdown,
/// writing output to `<config_dir>/rag/docs/rust/`.
pub fn generate_docs(config_dir: &Path) -> Result<()> {
    let output_dir = config_dir.join("rag").join("docs").join("rust");
    info!(output_dir = %output_dir.display(), "generating RAG docs");

    // Verify cargo-docs-md is available
    let which = Command::new("which")
        .arg("cargo-docs-md")
        .output()
        .context("failed to check for cargo-docs-md")?;
    if !which.status.success() {
        bail!("cargo-docs-md not found — run `mise install` in the ur repo");
    }
    debug!("cargo-docs-md found");

    // Step 1: collect sources
    println!("Collecting sources...");
    let collect = Command::new("cargo-docs-md")
        .arg("docs-md")
        .arg("collect-sources")
        .status()
        .context("failed to run cargo-docs-md collect-sources")?;
    if !collect.success() {
        bail!(
            "cargo-docs-md collect-sources failed (exit code: {:?})",
            collect.code()
        );
    }
    info!("collect-sources completed");

    // Step 2: generate docs
    println!("Generating docs to {}...", output_dir.display());
    let docs = Command::new("cargo-docs-md")
        .arg("docs-md")
        .arg("docs")
        .arg("--output")
        .arg(&output_dir)
        .status()
        .context("failed to run cargo-docs-md docs")?;
    if !docs.success() {
        bail!("cargo-docs-md docs failed (exit code: {:?})", docs.code());
    }

    info!(output_dir = %output_dir.display(), "RAG docs generated");
    println!("Done. Docs written to {}", output_dir.display());
    Ok(())
}

fn parse_language(s: &str) -> Result<Language> {
    match s.to_lowercase().as_str() {
        "rust" => Ok(Language::Rust),
        other => bail!("unsupported language: {other}"),
    }
}

/// Send a RagIndex gRPC request to ur-server.
pub async fn index(port: u16, language: &str) -> Result<()> {
    let lang = parse_language(language)?;

    let addr = format!("http://127.0.0.1:{port}");
    let mut client = RagServiceClient::connect(addr)
        .await
        .context("failed to connect to ur-server — is it running? Try 'ur start'")?;

    info!(language = %language, "sending RagIndex request");
    println!("Indexing {language} docs...");

    let resp = client
        .rag_index(RagIndexRequest {
            language: lang.into(),
        })
        .await
        .map_err(|status| {
            if status.message().to_lowercase().contains("no docs found")
                || status.message().to_lowercase().contains("empty")
            {
                anyhow::anyhow!(
                    "No docs found for language '{language}'. Run `ur rag docs` first to generate documentation."
                )
            } else {
                anyhow::anyhow!("RagIndex failed: {status}")
            }
        })?;

    let inner = resp.into_inner();
    println!(
        "Indexed {} files, {} chunks",
        inner.files_processed, inner.chunks_indexed
    );
    Ok(())
}

/// Send a RagSearch gRPC request to ur-server.
pub async fn search(port: u16, query: &str, language: &str, top_k: u32) -> Result<()> {
    let lang = parse_language(language)?;

    let addr = format!("http://127.0.0.1:{port}");
    let mut client = RagServiceClient::connect(addr)
        .await
        .context("failed to connect to ur-server — is it running? Try 'ur start'")?;

    info!(query = %query, language = %language, top_k, "sending RagSearch request");

    let resp = client
        .rag_search(RagSearchRequest {
            query: query.to_owned(),
            language: lang.into(),
            top_k: Some(top_k),
        })
        .await
        .context("RagSearch failed")?;

    let results = resp.into_inner().results;
    if results.is_empty() {
        println!("No results found.");
        return Ok(());
    }

    for (i, result) in results.iter().enumerate() {
        println!("--- Result {} (score: {:.4}) ---", i + 1, result.score);
        println!("Source: {}", result.source_file);
        println!("{}", result.text);
        println!();
    }

    Ok(())
}

/// Download the configured embedding model to the local fastembed cache.
///
/// Uses curl to fetch model files from HuggingFace, matching the hf_hub
/// cache layout that fastembed expects.
pub fn download_model(config: &ur_config::Config) -> Result<()> {
    let model_name = &config.rag.embedding_model;
    let info = ur_config::model_download_info(model_name).ok_or_else(|| {
        let supported = ur_config::supported_model_names().join(", ");
        anyhow::anyhow!("unknown embedding model '{model_name}' — supported models: {supported}")
    })?;

    let cache_dir = config.config_dir.join("fastembed");
    let model_dir = cache_dir.join(format!("models--{}--{}", info.hf_org, info.hf_repo));
    let snapshot_dir = model_dir.join("snapshots").join(info.hf_commit);

    // Check if already downloaded
    if snapshot_dir.exists() {
        let all_present = info.hf_files.iter().all(|f| snapshot_dir.join(f).exists());
        if all_present {
            println!(
                "Model '{model_name}' already downloaded at {}",
                cache_dir.display()
            );
            return Ok(());
        }
    }

    println!(
        "Downloading model '{model_name}' to {}...",
        cache_dir.display()
    );

    std::fs::create_dir_all(model_dir.join("refs"))
        .context("failed to create model refs directory")?;
    std::fs::create_dir_all(model_dir.join("blobs"))
        .context("failed to create model blobs directory")?;
    std::fs::create_dir_all(&snapshot_dir).context("failed to create model snapshot directory")?;

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
            bail!(
                "curl failed to download {url} (exit code: {:?})",
                status.code()
            );
        }
    }

    println!(
        "Done. Model '{}' cached at {}",
        model_name,
        cache_dir.display()
    );
    Ok(())
}
