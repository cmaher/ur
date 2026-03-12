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
        .arg("collect-sources")
        .status()
        .context("failed to run cargo-docs-md collect-sources")?;
    if !collect.success() {
        bail!("cargo-docs-md collect-sources failed (exit code: {:?})", collect.code());
    }
    info!("collect-sources completed");

    // Step 2: generate docs
    println!("Generating docs to {}...", output_dir.display());
    let docs = Command::new("cargo-docs-md")
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
