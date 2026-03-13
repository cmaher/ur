use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Per-file entry in the index manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileEntry {
    pub hash: String,
    pub chunk_ids: Vec<Uuid>,
}

/// Persistent manifest tracking which files have been indexed and their content hashes.
///
/// Stored at `~/.ur/rag/index-manifest-{language}.json`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IndexManifest {
    /// Name of the embedding model used to generate the indexed vectors.
    pub model: String,
    /// Map from relative file path to its entry (hash + chunk IDs).
    pub files: HashMap<String, FileEntry>,
}

impl IndexManifest {
    /// Load the manifest from disk, or return a fresh empty one if the file doesn't exist.
    pub fn load(language: &str, model_name: &str) -> Result<Self> {
        let path = manifest_path(language)?;
        if !path.exists() {
            return Ok(Self {
                model: model_name.to_string(),
                files: HashMap::new(),
            });
        }
        let data =
            std::fs::read_to_string(&path).context("Failed to read index manifest from disk")?;
        let manifest: Self =
            serde_json::from_str(&data).context("Failed to parse index manifest JSON")?;
        Ok(manifest)
    }

    /// Persist the manifest to disk, creating parent directories if needed.
    pub fn save(&self, language: &str) -> Result<()> {
        let path = manifest_path(language)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("Failed to create manifest directory")?;
        }
        let data =
            serde_json::to_string_pretty(self).context("Failed to serialize index manifest")?;
        std::fs::write(&path, data).context("Failed to write index manifest to disk")?;
        Ok(())
    }
}

/// Compute the SHA-256 hex digest of a file's contents.
pub fn sha256_file(path: &Path) -> Result<String> {
    let data = std::fs::read(path)
        .with_context(|| format!("Failed to read file for hashing: {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let result = hasher.finalize();
    Ok(result.iter().map(|b| format!("{b:02x}")).collect())
}

/// Resolve the manifest file path: `$UR_CONFIG/rag/index-manifest-{language}.json`.
fn manifest_path(language: &str) -> Result<PathBuf> {
    let config_dir = ur_config::resolve_config_dir()?;
    Ok(config_dir
        .join("rag")
        .join(format!("index-manifest-{language}.json")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn sha256_computes_correct_hash() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "hello world").unwrap();

        let hash = sha256_file(&file).unwrap();
        // Known SHA-256 of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn manifest_roundtrip_serde() {
        let manifest = IndexManifest {
            model: "test-model".to_string(),
            files: HashMap::from([(
                "foo/bar.md".to_string(),
                FileEntry {
                    hash: "abc123".to_string(),
                    chunk_ids: vec![Uuid::new_v4()],
                },
            )]),
        };

        let json = serde_json::to_string(&manifest).unwrap();
        let parsed: IndexManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.model, "test-model");
        assert_eq!(parsed.files.len(), 1);
        assert!(parsed.files.contains_key("foo/bar.md"));
    }
}
