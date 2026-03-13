use std::path::Path;

use anyhow::{Context, Result};
use text_splitter::MarkdownSplitter;
use std::collections::BTreeMap;
use tracing::{debug, info};
use walkdir::WalkDir;

/// A chunk of text extracted from a markdown file.
#[derive(Debug)]
pub struct DocChunk {
    pub text: String,
    pub source_file: String,
}

/// Read all markdown files from `docs_dir` and split them into semantic chunks.
///
/// Uses `text-splitter` with markdown-aware boundaries, targeting 500-1500 character chunks.
pub fn read_and_chunk_docs(docs_dir: &Path) -> Result<Vec<DocChunk>> {
    let splitter = MarkdownSplitter::new(500..1500);
    let mut chunks = Vec::new();

    let entries: Vec<_> = WalkDir::new(docs_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .is_some_and(|ext| ext == "md" || ext == "markdown")
        })
        .collect();

    if entries.is_empty() {
        anyhow::bail!(
            "No markdown files found in {}. Run `ur rag docs` first to generate documentation.",
            docs_dir.display()
        );
    }

    // Group files by top-level subdirectory (each is a dependency)
    let mut deps: BTreeMap<String, Vec<&walkdir::DirEntry>> = BTreeMap::new();
    for entry in &entries {
        let relative = entry.path().strip_prefix(docs_dir).unwrap_or(entry.path());
        let dep_name = relative
            .components()
            .next()
            .map(|c| c.as_os_str().to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        deps.entry(dep_name).or_default().push(entry);
    }

    for (dep_name, dep_entries) in &deps {
        let mut dep_files = 0u64;
        let mut dep_chunks = 0u64;

        for entry in dep_entries {
            let path = entry.path();
            let content = std::fs::read_to_string(path)
                .with_context(|| format!("Failed to read {}", path.display()))?;

            if content.trim().is_empty() {
                continue;
            }

            let relative = path
                .strip_prefix(docs_dir)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let file_chunks: Vec<&str> = splitter.chunks(&content).collect();
            debug!(
                file = %relative,
                chunk_count = file_chunks.len(),
                "chunked markdown file"
            );

            dep_files += 1;
            dep_chunks += file_chunks.len() as u64;

            for chunk_text in file_chunks {
                chunks.push(DocChunk {
                    text: chunk_text.to_string(),
                    source_file: relative.clone(),
                });
            }
        }

        info!("indexed {dep_name}: {dep_files} files, {dep_chunks} chunks");
    }

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn chunks_single_markdown_file() {
        let dir = TempDir::new().unwrap();
        let content = "# Header\n\n".to_string()
            + &"This is a paragraph with enough content to form a chunk. ".repeat(20)
            + "\n\n## Second Section\n\n"
            + &"Another paragraph with different content for the second section. ".repeat(20);
        fs::write(dir.path().join("test.md"), &content).unwrap();

        let chunks = read_and_chunk_docs(dir.path()).unwrap();
        assert!(!chunks.is_empty(), "should produce at least one chunk");
        for chunk in &chunks {
            assert_eq!(chunk.source_file, "test.md");
            assert!(!chunk.text.is_empty());
        }
    }

    #[test]
    fn skips_non_markdown_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("readme.txt"), "not markdown").unwrap();
        fs::write(
            dir.path().join("doc.md"),
            "Some markdown content. ".repeat(30),
        )
        .unwrap();

        let chunks = read_and_chunk_docs(dir.path()).unwrap();
        assert!(chunks.iter().all(|c| c.source_file == "doc.md"));
    }

    #[test]
    fn errors_on_empty_directory() {
        let dir = TempDir::new().unwrap();
        let result = read_and_chunk_docs(dir.path());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No markdown files")
        );
    }

    #[test]
    fn handles_nested_directories() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("sub").join("deep");
        fs::create_dir_all(&sub).unwrap();
        fs::write(
            sub.join("nested.md"),
            "Nested document content here. ".repeat(30),
        )
        .unwrap();

        let chunks = read_and_chunk_docs(dir.path()).unwrap();
        assert!(!chunks.is_empty());
        assert!(chunks[0].source_file.contains("nested.md"));
        assert!(chunks[0].source_file.contains("sub"));
    }

    #[test]
    fn skips_empty_markdown_files() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("empty.md"), "   \n\n  ").unwrap();
        fs::write(dir.path().join("real.md"), "Real content here. ".repeat(30)).unwrap();

        let chunks = read_and_chunk_docs(dir.path()).unwrap();
        assert!(chunks.iter().all(|c| c.source_file == "real.md"));
    }

    #[test]
    fn chunk_sizes_within_bounds() {
        let dir = TempDir::new().unwrap();
        // Create a large document that will produce multiple chunks
        let mut content = String::new();
        for i in 0..50 {
            content.push_str(&format!("## Section {i}\n\n"));
            content.push_str(&format!(
                "This is section {i} with enough content to be meaningful. "
            ));
            content.push_str(
                "We need to add more text here to ensure chunks are generated properly. ",
            );
            content.push_str(
                "The text splitter should respect markdown boundaries when splitting. \n\n",
            );
        }
        fs::write(dir.path().join("large.md"), &content).unwrap();

        let chunks = read_and_chunk_docs(dir.path()).unwrap();
        assert!(
            chunks.len() > 1,
            "large document should produce multiple chunks"
        );
        // text-splitter targets the range but may produce slightly smaller chunks at boundaries
        for chunk in &chunks {
            assert!(
                chunk.text.len() <= 2000,
                "chunk too large: {} chars",
                chunk.text.len()
            );
        }
    }
}
