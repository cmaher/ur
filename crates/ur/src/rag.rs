use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};

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
