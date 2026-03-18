use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use fastembed::TextEmbedding;
use qdrant_client::Payload;
use qdrant_client::qdrant::{
    CreateCollectionBuilder, DeletePointsBuilder, Distance, PointStruct, PointsIdsList,
    SearchPointsBuilder, UpsertPointsBuilder, VectorParamsBuilder,
};
use tracing::{debug, info};
use uuid::Uuid;

use crate::chunking;
use crate::manifest::{self, FileEntry, IndexManifest};

const UPSERT_BATCH_SIZE: usize = 256;
const EMBED_BATCH_SIZE: usize = 32;
const DEFAULT_TOP_K: u64 = 5;

/// Progress update for a single dependency that was indexed.
pub struct DependencyProgress {
    pub name: String,
    pub files: u32,
    pub chunks: u32,
}

/// Summary of a completed indexing operation.
pub struct IndexSummary {
    pub files_processed: u32,
    pub chunks_indexed: u32,
}

/// A single search result from the RAG system.
pub struct SearchResult {
    pub text: String,
    pub source_file: String,
    pub score: f32,
}

/// Manages RAG indexing and search operations against Qdrant.
///
/// Accepts a Qdrant client and a fastembed model via constructor (dependency injection).
/// Implements `Clone` — the Qdrant client and embedding model are shared via `Arc`.
#[derive(Clone)]
pub struct RagManager {
    qdrant: Arc<qdrant_client::Qdrant>,
    embedding_model: Arc<TextEmbedding>,
    vector_size: u64,
    model_name: String,
}

impl RagManager {
    pub fn new(
        qdrant: Arc<qdrant_client::Qdrant>,
        embedding_model: Arc<TextEmbedding>,
        vector_size: u64,
        model_name: String,
    ) -> Self {
        Self {
            qdrant,
            embedding_model,
            vector_size,
            model_name,
        }
    }

    /// Index markdown documents from `docs_dir` into a language-specific Qdrant collection.
    ///
    /// Uses a local manifest to perform incremental indexing: only changed/new files are
    /// embedded and upserted, unchanged files are skipped, and removed files have their
    /// points deleted. A model name change triggers a full re-index.
    ///
    /// If `progress_tx` is provided, sends a `DependencyProgress` for each dependency
    /// as it finishes indexing.
    pub async fn index(
        &self,
        docs_dir: &Path,
        language: &str,
        progress_tx: Option<tokio::sync::mpsc::Sender<DependencyProgress>>,
    ) -> Result<IndexSummary> {
        let collection = collection_name(language);

        // Read and chunk all documents from disk
        let all_chunks = chunking::read_and_chunk_docs(docs_dir)?;
        let total_files = count_unique_files(&all_chunks);

        // Load the existing manifest (or create a fresh one)
        let mut prev_manifest = IndexManifest::load(language, &self.model_name)?;

        // If the model changed, force a full re-index by clearing the manifest
        let model_changed = prev_manifest.model != self.model_name;
        if model_changed {
            info!(
                old_model = %prev_manifest.model,
                new_model = %self.model_name,
                "embedding model changed, triggering full re-index"
            );
            // Delete all old points if there's an existing collection
            self.ensure_collection_exists(&collection).await?;
            self.recreate_collection(&collection).await?;
            prev_manifest.files.clear();
            prev_manifest.model = self.model_name.clone();
        } else {
            // Ensure the collection exists (create if missing, don't drop)
            self.ensure_collection_exists(&collection).await?;
        }

        let chunks_by_file = group_chunks_by_file(&all_chunks);
        let current_files: HashSet<String> = chunks_by_file.keys().cloned().collect();
        let current_hashes = hash_files(docs_dir, &current_files)?;

        let (files_to_index, files_to_delete) =
            compute_index_diff(&current_files, &current_hashes, &prev_manifest);

        info!(
            collection = %collection,
            total_files = total_files,
            changed = files_to_index.len(),
            removed = files_to_delete.len(),
            skipped = total_files - files_to_index.len(),
            "incremental indexing"
        );

        self.delete_stale_points(
            &collection,
            &mut prev_manifest,
            &files_to_delete,
            &files_to_index,
        )
        .await?;

        // Group files to index by dependency (first path component)
        let mut files_by_dep: std::collections::BTreeMap<String, Vec<String>> =
            std::collections::BTreeMap::new();
        for file_path in &files_to_index {
            let dep_name = file_path.split('/').next().unwrap_or("unknown").to_string();
            files_by_dep
                .entry(dep_name)
                .or_default()
                .push(file_path.clone());
        }

        let mut chunks_indexed: usize = 0;

        // Process each dependency's files together and report progress
        for (dep_name, dep_files) in &files_by_dep {
            let chunks_to_embed: Vec<(&chunking::DocChunk, String)> = dep_files
                .iter()
                .flat_map(|file_path| {
                    chunks_by_file
                        .get(file_path)
                        .into_iter()
                        .flatten()
                        .map(move |chunk| (*chunk, file_path.clone()))
                })
                .collect();

            let dep_chunk_count = chunks_to_embed.len();
            chunks_indexed += dep_chunk_count;

            if !chunks_to_embed.is_empty() {
                let new_chunk_ids = self
                    .embed_and_upsert(&collection, language, &chunks_to_embed)
                    .await?;

                update_manifest(&mut prev_manifest, &new_chunk_ids, &current_hashes);
            }

            if let Some(tx) = &progress_tx {
                let _ = tx
                    .send(DependencyProgress {
                        name: dep_name.clone(),
                        files: dep_files.len() as u32,
                        chunks: dep_chunk_count as u32,
                    })
                    .await;
            }
        }

        // Save the updated manifest
        prev_manifest.save(language)?;

        info!(
            collection = %collection,
            total_files = total_files,
            chunks_indexed = chunks_indexed,
            "indexing complete"
        );

        Ok(IndexSummary {
            files_processed: total_files as u32,
            chunks_indexed: chunks_indexed as u32,
        })
    }

    /// Search the language-specific collection for chunks matching the query.
    ///
    /// Returns up to `top_k` results (default 5) sorted by relevance.
    pub async fn search(
        &self,
        query: &str,
        language: &str,
        top_k: Option<u64>,
    ) -> Result<Vec<SearchResult>> {
        let collection = collection_name(language);
        let limit = top_k.unwrap_or(DEFAULT_TOP_K);

        // Embed the query
        let model = Arc::clone(&self.embedding_model);
        let query_text = query.to_string();
        let embeddings = tokio::task::spawn_blocking(move || model.embed(vec![query_text], None))
            .await
            .context("Embedding task panicked")?
            .context("Failed to embed search query")?;

        let query_vector = embeddings
            .into_iter()
            .next()
            .context("Embedding model returned no vectors")?;

        let response = self
            .qdrant
            .search_points(
                SearchPointsBuilder::new(&collection, query_vector, limit).with_payload(true),
            )
            .await
            .context("Failed to search Qdrant")?;

        let results = response
            .result
            .into_iter()
            .map(|point| {
                let text = extract_string_payload(&point.payload, "text");
                let source_file = extract_string_payload(&point.payload, "source_file");

                SearchResult {
                    text,
                    source_file,
                    score: point.score,
                }
            })
            .collect();

        Ok(results)
    }

    /// Embed a set of chunks, upsert them to Qdrant, and return the chunk IDs grouped by file.
    async fn embed_and_upsert(
        &self,
        collection: &str,
        language: &str,
        chunks: &[(&chunking::DocChunk, String)],
    ) -> Result<std::collections::HashMap<String, Vec<Uuid>>> {
        // Embed in small batches
        let texts: Vec<String> = chunks.iter().map(|(c, _)| c.text.clone()).collect();
        let mut embeddings = Vec::with_capacity(texts.len());
        for batch in texts.chunks(EMBED_BATCH_SIZE) {
            let batch_vec = batch.to_vec();
            let model = Arc::clone(&self.embedding_model);
            let batch_embeddings =
                tokio::task::spawn_blocking(move || model.embed(batch_vec, None))
                    .await
                    .context("Embedding task panicked")?
                    .context("Failed to embed document chunks")?;
            embeddings.extend(batch_embeddings);
        }

        let mut chunk_ids_by_file: std::collections::HashMap<String, Vec<Uuid>> =
            std::collections::HashMap::new();

        // Upsert in batches (last batch waits synchronously to ensure all writes complete)
        let total = chunks.len();
        for batch_start in (0..total).step_by(UPSERT_BATCH_SIZE) {
            let batch_end = (batch_start + UPSERT_BATCH_SIZE).min(total);
            let is_last_batch = batch_end == total;
            let points: Vec<PointStruct> = (batch_start..batch_end)
                .map(|i| {
                    let (chunk, file_path) = &chunks[i];
                    let point_id = Uuid::new_v4();
                    chunk_ids_by_file
                        .entry(file_path.clone())
                        .or_default()
                        .push(point_id);

                    let mut payload = Payload::new();
                    payload.insert("text", chunk.text.as_str());
                    payload.insert("source_file", chunk.source_file.as_str());
                    payload.insert("language", language);

                    PointStruct::new(point_id.to_string(), embeddings[i].clone(), payload)
                })
                .collect();

            self.qdrant
                .upsert_points(UpsertPointsBuilder::new(collection, points).wait(is_last_batch))
                .await
                .context("Failed to upsert points to Qdrant")?;

            debug!(batch_start, batch_end, "upserted batch to Qdrant");
        }

        Ok(chunk_ids_by_file)
    }

    /// Ensure the collection exists, creating it if missing.
    async fn ensure_collection_exists(&self, collection_name: &str) -> Result<()> {
        let exists = self
            .qdrant
            .collection_exists(collection_name)
            .await
            .context("Failed to check collection existence")?;

        if !exists {
            self.qdrant
                .create_collection(
                    CreateCollectionBuilder::new(collection_name).vectors_config(
                        VectorParamsBuilder::new(self.vector_size, Distance::Cosine),
                    ),
                )
                .await
                .context("Failed to create collection")?;
            info!(collection = %collection_name, "created collection");
        }

        Ok(())
    }

    /// Drop the collection if it exists, then create a fresh one with the correct vector config.
    async fn recreate_collection(&self, collection_name: &str) -> Result<()> {
        let exists = self
            .qdrant
            .collection_exists(collection_name)
            .await
            .context("Failed to check collection existence")?;

        if exists {
            self.qdrant
                .delete_collection(collection_name)
                .await
                .context("Failed to delete existing collection")?;
            debug!(collection = %collection_name, "deleted existing collection");
        }

        self.qdrant
            .create_collection(
                CreateCollectionBuilder::new(collection_name)
                    .vectors_config(VectorParamsBuilder::new(self.vector_size, Distance::Cosine)),
            )
            .await
            .context("Failed to create collection")?;

        info!(collection = %collection_name, "created collection");
        Ok(())
    }

    async fn delete_stale_points(
        &self,
        collection: &str,
        prev_manifest: &mut IndexManifest,
        files_to_delete: &[String],
        files_to_index: &[String],
    ) -> Result<()> {
        for file_path in files_to_delete {
            if let Some(entry) = prev_manifest.files.remove(file_path) {
                self.delete_points_by_ids(collection, &entry.chunk_ids)
                    .await?;
                debug!(file = %file_path, chunks = entry.chunk_ids.len(), "deleted removed file points");
            }
        }

        for file_path in files_to_index {
            if let Some(entry) = prev_manifest.files.remove(file_path) {
                self.delete_points_by_ids(collection, &entry.chunk_ids)
                    .await?;
                debug!(file = %file_path, chunks = entry.chunk_ids.len(), "deleted stale file points");
            }
        }

        Ok(())
    }

    /// Delete specific points from a collection by their UUIDs.
    async fn delete_points_by_ids(&self, collection_name: &str, chunk_ids: &[Uuid]) -> Result<()> {
        if chunk_ids.is_empty() {
            return Ok(());
        }
        let ids: Vec<qdrant_client::qdrant::PointId> =
            chunk_ids.iter().map(|id| id.to_string().into()).collect();
        self.qdrant
            .delete_points(
                DeletePointsBuilder::new(collection_name)
                    .points(PointsIdsList { ids })
                    .wait(true),
            )
            .await
            .context("Failed to delete points from Qdrant")?;
        Ok(())
    }
}

fn group_chunks_by_file(
    chunks: &[chunking::DocChunk],
) -> std::collections::HashMap<String, Vec<&chunking::DocChunk>> {
    let mut map: std::collections::HashMap<String, Vec<&chunking::DocChunk>> =
        std::collections::HashMap::new();
    for chunk in chunks {
        map.entry(chunk.source_file.clone())
            .or_default()
            .push(chunk);
    }
    map
}

fn hash_files(
    docs_dir: &Path,
    files: &HashSet<String>,
) -> Result<std::collections::HashMap<String, String>> {
    let mut hashes = std::collections::HashMap::new();
    for file_path in files {
        let full_path = docs_dir.join(file_path);
        let hash = manifest::sha256_file(&full_path)?;
        hashes.insert(file_path.clone(), hash);
    }
    Ok(hashes)
}

fn compute_index_diff(
    current_files: &HashSet<String>,
    current_hashes: &std::collections::HashMap<String, String>,
    prev_manifest: &IndexManifest,
) -> (Vec<String>, Vec<String>) {
    let mut files_to_index: Vec<String> = Vec::new();
    for file_path in current_files {
        let current_hash = &current_hashes[file_path];
        match prev_manifest.files.get(file_path) {
            Some(entry) if entry.hash == *current_hash => {
                debug!(file = %file_path, "unchanged, skipping");
            }
            _ => {
                files_to_index.push(file_path.clone());
            }
        }
    }

    let mut files_to_delete: Vec<String> = Vec::new();
    for file_path in prev_manifest.files.keys() {
        if !current_files.contains(file_path) {
            files_to_delete.push(file_path.clone());
        }
    }

    (files_to_index, files_to_delete)
}

/// Update the manifest with newly indexed file entries.
fn update_manifest(
    manifest: &mut IndexManifest,
    chunk_ids_by_file: &std::collections::HashMap<String, Vec<Uuid>>,
    current_hashes: &std::collections::HashMap<String, String>,
) {
    for (file_path, chunk_ids) in chunk_ids_by_file {
        manifest.files.insert(
            file_path.clone(),
            FileEntry {
                hash: current_hashes[file_path].clone(),
                chunk_ids: chunk_ids.clone(),
            },
        );
    }
}

/// Build the collection name for a given language (e.g. "rust" -> "rag_docs_rust").
fn collection_name(language: &str) -> String {
    format!("rag_docs_{language}")
}

/// Count the number of unique source files in a set of chunks.
fn count_unique_files(chunks: &[chunking::DocChunk]) -> usize {
    let mut files: Vec<&str> = chunks.iter().map(|c| c.source_file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    files.len()
}

/// Extract a string value from a Qdrant point payload.
fn extract_string_payload(
    payload: &std::collections::HashMap<String, qdrant_client::qdrant::Value>,
    key: &str,
) -> String {
    payload
        .get(key)
        .and_then(|v| match &v.kind {
            Some(qdrant_client::qdrant::value::Kind::StringValue(s)) => Some(s.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_name_formats_correctly() {
        assert_eq!(collection_name("rust"), "rag_docs_rust");
        assert_eq!(collection_name("python"), "rag_docs_python");
    }

    #[test]
    fn count_unique_files_deduplicates() {
        let chunks = vec![
            chunking::DocChunk {
                text: "a".into(),
                source_file: "file1.md".into(),
            },
            chunking::DocChunk {
                text: "b".into(),
                source_file: "file1.md".into(),
            },
            chunking::DocChunk {
                text: "c".into(),
                source_file: "file2.md".into(),
            },
        ];
        assert_eq!(count_unique_files(&chunks), 2);
    }

    #[test]
    fn extract_string_from_payload() {
        use qdrant_client::qdrant::{Value, value::Kind};
        let mut payload = std::collections::HashMap::new();
        payload.insert(
            "text".to_string(),
            Value {
                kind: Some(Kind::StringValue("hello".to_string())),
            },
        );
        assert_eq!(extract_string_payload(&payload, "text"), "hello");
        assert_eq!(extract_string_payload(&payload, "missing"), "");
    }
}
