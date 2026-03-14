use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use tracing::{debug, info};
use ur_rpc::proto::rag::rag_service_client::RagServiceClient;
use ur_rpc::proto::rag::{Language, RagIndexRequest, RagSearchRequest};

/// Generate Rust documentation into the RAG docs directory.
///
/// Runs `cargo +nightly doc` to produce rustdoc JSON for the full dependency
/// tree, filters the JSON to workspace crates and direct dependencies only,
/// then converts the filtered JSON to markdown via `cargo-docs-md`.
pub fn generate_docs(config: &ur_config::Config) -> Result<()> {
    let output_dir = config.config_dir.join("rag").join("docs").join("rust");
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

    let workspace_root = find_workspace_root()?;

    // Step 1: generate rustdoc JSON for all crates
    println!("Generating rustdoc JSON...");
    let cargo_doc = Command::new("cargo")
        .arg("+nightly")
        .arg("doc")
        .env("RUSTDOCFLAGS", "-Z unstable-options --output-format json")
        .current_dir(&workspace_root)
        .status()
        .context("failed to run cargo +nightly doc")?;
    if !cargo_doc.success() {
        bail!(
            "cargo +nightly doc failed (exit code: {:?})",
            cargo_doc.code()
        );
    }

    // Step 2: filter JSON to workspace crates + direct dependencies only
    let allowed = collect_allowed_crates(&workspace_root, &config.rag.docs.exclude)?;
    let json_src = workspace_root.join("target").join("doc");
    let json_filtered = workspace_root.join("target").join("doc-rag-filtered");

    if json_filtered.exists() {
        std::fs::remove_dir_all(&json_filtered)
            .context("failed to clean filtered JSON directory")?;
    }
    std::fs::create_dir_all(&json_filtered)
        .context("failed to create filtered JSON directory")?;

    let mut copied = 0;
    for name in &allowed {
        let src = json_src.join(format!("{name}.json"));
        if src.exists() {
            std::fs::copy(&src, json_filtered.join(format!("{name}.json")))
                .with_context(|| format!("failed to copy {}", src.display()))?;
            copied += 1;
        }
    }

    info!(
        allowed = allowed.len(),
        copied,
        "filtered rustdoc JSON to allowed crates"
    );
    println!("Generating docs for {copied} crates to {}...", output_dir.display());

    // Step 3: generate markdown from filtered JSON
    let docs = Command::new("cargo-docs-md")
        .arg("docs-md")
        .arg("--dir")
        .arg(&json_filtered)
        .arg("--output")
        .arg(&output_dir)
        .status()
        .context("failed to run cargo-docs-md")?;
    if !docs.success() {
        bail!("cargo-docs-md failed (exit code: {:?})", docs.code());
    }

    let _ = std::fs::remove_dir_all(&json_filtered);

    info!(output_dir = %output_dir.display(), "RAG docs generated");
    println!("Done. Docs written to {}", output_dir.display());
    Ok(())
}

/// Find the workspace root by looking for the top-level `Cargo.toml` with a
/// `[workspace]` section, starting from the current directory and walking up.
fn find_workspace_root() -> Result<std::path::PathBuf> {
    let mut dir = std::env::current_dir().context("failed to get current directory")?;
    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let contents =
                std::fs::read_to_string(&cargo_toml).context("failed to read Cargo.toml")?;
            if contents.contains("[workspace]") {
                return Ok(dir);
            }
        }
        if !dir.pop() {
            bail!("could not find workspace root (no Cargo.toml with [workspace] found)");
        }
    }
}

/// Collect workspace crate names and their direct dependency names into a single
/// set. Crate names use underscores (matching cargo-docs-md directory names).
fn collect_allowed_crates(workspace_root: &Path, exclude: &[String]) -> Result<HashSet<String>> {
    let exclude_set: HashSet<&str> = exclude.iter().map(String::as_str).collect();
    let mut allowed = HashSet::new();

    // Parse workspace Cargo.toml to get member paths
    let ws_toml_path = workspace_root.join("Cargo.toml");
    let ws_contents =
        std::fs::read_to_string(&ws_toml_path).context("failed to read workspace Cargo.toml")?;
    let ws_doc: toml::Value =
        toml::from_str(&ws_contents).context("failed to parse workspace Cargo.toml")?;

    let members = ws_doc
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(|m| m.as_array())
        .ok_or_else(|| anyhow::anyhow!("workspace Cargo.toml missing [workspace].members"))?;

    // Resolve member globs and collect each member's crate name + direct deps
    for member_val in members {
        let pattern = member_val
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("workspace member is not a string"))?;

        let full_pattern = workspace_root.join(pattern);
        let pattern_str = full_pattern.to_str().ok_or_else(|| {
            anyhow::anyhow!("non-UTF8 workspace member path: {}", full_pattern.display())
        })?;

        let paths = glob::glob(pattern_str)
            .with_context(|| format!("invalid workspace member glob: {pattern}"))?;

        for path_result in paths {
            let member_dir = path_result
                .with_context(|| format!("error expanding workspace member glob: {pattern}"))?;
            let member_toml_path = member_dir.join("Cargo.toml");
            if !member_toml_path.exists() {
                continue;
            }

            let member_contents =
                std::fs::read_to_string(&member_toml_path).with_context(|| {
                    format!(
                        "failed to read member Cargo.toml: {}",
                        member_toml_path.display()
                    )
                })?;
            let member_doc: toml::Value = toml::from_str(&member_contents).with_context(|| {
                format!(
                    "failed to parse member Cargo.toml: {}",
                    member_toml_path.display()
                )
            })?;

            // Add the crate's own name (with hyphens converted to underscores)
            if let Some(name) = member_doc
                .get("package")
                .and_then(|p| p.get("name"))
                .and_then(|n| n.as_str())
            {
                allowed.insert(name.replace('-', "_"));
            }

            // Add direct [dependencies] names (not dev-dependencies or build-dependencies)
            collect_direct_deps(&member_doc, &exclude_set, &mut allowed);
        }
    }

    Ok(allowed)
}

fn collect_direct_deps(
    member_doc: &toml::Value,
    exclude_set: &HashSet<&str>,
    allowed: &mut HashSet<String>,
) {
    let Some(deps) = member_doc.get("dependencies").and_then(|d| d.as_table()) else {
        return;
    };
    for dep_name in deps.keys() {
        let normalized = dep_name.replace('-', "_");
        if !exclude_set.contains(dep_name.as_str()) && !exclude_set.contains(normalized.as_str()) {
            allowed.insert(normalized);
        }
    }
}

fn parse_language(s: &str) -> Result<Language> {
    match s.to_lowercase().as_str() {
        "rust" => Ok(Language::Rust),
        other => bail!("unsupported language: {other}"),
    }
}

/// Send a RagIndex gRPC request to ur-server and stream progress.
pub async fn index(port: u16, language: &str) -> Result<()> {
    use ur_rpc::proto::rag::rag_index_progress::Update;

    let lang = parse_language(language)?;

    let addr = format!("http://127.0.0.1:{port}");
    let mut client = RagServiceClient::connect(addr)
        .await
        .context("failed to connect to ur-server — is it running? Try 'ur start'")?;

    info!(language = %language, "sending RagIndex request");
    println!("Indexing {language} docs...");

    let mut stream = client
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
        })?
        .into_inner();

    while let Some(msg) = stream.message().await.context("stream error")? {
        match msg.update {
            Some(Update::DependencyIndexed(dep)) => {
                println!(
                    "  {} — {} files, {} chunks",
                    dep.name, dep.files, dep.chunks
                );
            }
            Some(Update::IndexComplete(complete)) => {
                println!(
                    "Done. Indexed {} files, {} chunks total.",
                    complete.total_files, complete.total_chunks
                );
            }
            None => {}
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Create a minimal workspace layout in a temp dir for testing.
    fn create_test_workspace(tmp: &TempDir) -> std::path::PathBuf {
        let root = tmp.path().to_path_buf();

        // Workspace Cargo.toml
        std::fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/alpha", "crates/beta"]
"#,
        )
        .unwrap();

        // crate alpha: depends on serde and tokio
        let alpha_dir = root.join("crates").join("alpha");
        std::fs::create_dir_all(&alpha_dir).unwrap();
        std::fs::write(
            alpha_dir.join("Cargo.toml"),
            r#"
[package]
name = "alpha"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1"
tokio = "1"
"#,
        )
        .unwrap();

        // crate beta: depends on anyhow and serde
        let beta_dir = root.join("crates").join("beta");
        std::fs::create_dir_all(&beta_dir).unwrap();
        std::fs::write(
            beta_dir.join("Cargo.toml"),
            r#"
[package]
name = "beta"
version = "0.1.0"
edition = "2021"

[dependencies]
anyhow = "1"
serde = "1"
"#,
        )
        .unwrap();

        root
    }

    #[test]
    fn collect_allowed_crates_includes_workspace_members_and_direct_deps() {
        let tmp = TempDir::new().unwrap();
        let root = create_test_workspace(&tmp);

        let allowed = collect_allowed_crates(&root, &[]).unwrap();

        // Workspace crate names
        assert!(allowed.contains("alpha"), "missing workspace crate alpha");
        assert!(allowed.contains("beta"), "missing workspace crate beta");

        // Direct dependencies
        assert!(allowed.contains("serde"), "missing direct dep serde");
        assert!(allowed.contains("tokio"), "missing direct dep tokio");
        assert!(allowed.contains("anyhow"), "missing direct dep anyhow");
    }

    #[test]
    fn collect_allowed_crates_excludes_listed_deps() {
        let tmp = TempDir::new().unwrap();
        let root = create_test_workspace(&tmp);

        let exclude = vec!["tokio".to_string()];
        let allowed = collect_allowed_crates(&root, &exclude).unwrap();

        // tokio should be excluded
        assert!(!allowed.contains("tokio"), "tokio should be excluded");
        // Others should still be present
        assert!(allowed.contains("alpha"));
        assert!(allowed.contains("serde"));
        assert!(allowed.contains("anyhow"));
    }

    #[test]
    fn collect_allowed_crates_normalizes_hyphens_to_underscores() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/my-crate"]
"#,
        )
        .unwrap();

        let crate_dir = root.join("crates").join("my-crate");
        std::fs::create_dir_all(&crate_dir).unwrap();
        std::fs::write(
            crate_dir.join("Cargo.toml"),
            r#"
[package]
name = "my-crate"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio-stream = "0.1"
"#,
        )
        .unwrap();

        let allowed = collect_allowed_crates(&root, &[]).unwrap();

        assert!(
            allowed.contains("my_crate"),
            "crate name should be normalized: {:?}",
            allowed
        );
        assert!(
            allowed.contains("tokio_stream"),
            "dep name should be normalized: {:?}",
            allowed
        );
    }

    #[test]
    fn collect_allowed_crates_handles_glob_members() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/*"]
"#,
        )
        .unwrap();

        let foo_dir = root.join("crates").join("foo");
        std::fs::create_dir_all(&foo_dir).unwrap();
        std::fs::write(
            foo_dir.join("Cargo.toml"),
            r#"
[package]
name = "foo"
version = "0.1.0"
edition = "2021"

[dependencies]
log = "0.4"
"#,
        )
        .unwrap();

        let bar_dir = root.join("crates").join("bar");
        std::fs::create_dir_all(&bar_dir).unwrap();
        std::fs::write(
            bar_dir.join("Cargo.toml"),
            r#"
[package]
name = "bar"
version = "0.1.0"
edition = "2021"

[dependencies]
clap = "4"
"#,
        )
        .unwrap();

        let allowed = collect_allowed_crates(&root, &[]).unwrap();
        assert!(allowed.contains("foo"));
        assert!(allowed.contains("bar"));
        assert!(allowed.contains("log"));
        assert!(allowed.contains("clap"));
    }

    #[test]
    fn exclude_works_with_hyphenated_names() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();

        std::fs::write(
            root.join("Cargo.toml"),
            r#"
[workspace]
members = ["crates/app"]
"#,
        )
        .unwrap();

        let app_dir = root.join("crates").join("app");
        std::fs::create_dir_all(&app_dir).unwrap();
        std::fs::write(
            app_dir.join("Cargo.toml"),
            r#"
[package]
name = "app"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio-stream = "0.1"
serde-json = "1"
"#,
        )
        .unwrap();

        // Exclude using hyphenated name
        let exclude = vec!["tokio-stream".to_string()];
        let allowed = collect_allowed_crates(&root, &exclude).unwrap();
        assert!(!allowed.contains("tokio_stream"));
        assert!(allowed.contains("serde_json"));

        // Exclude using underscored name
        let exclude2 = vec!["serde_json".to_string()];
        let allowed2 = collect_allowed_crates(&root, &exclude2).unwrap();
        assert!(allowed2.contains("tokio_stream"));
        assert!(!allowed2.contains("serde_json"));
    }
}
