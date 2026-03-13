use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use fastembed::TextEmbedding;
use qdrant_client::Payload;
use qdrant_client::qdrant::{
    CreateCollectionBuilder, Distance, PointStruct, SearchPointsBuilder, UpsertPointsBuilder,
    VectorParamsBuilder,
};
use tracing::{debug, info};
use uuid::Uuid;

use crate::chunking;

const UPSERT_BATCH_SIZE: usize = 256;
const EMBED_BATCH_SIZE: usize = 32;
const DEFAULT_TOP_K: u64 = 5;

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
}

impl RagManager {
    pub fn new(
        qdrant: Arc<qdrant_client::Qdrant>,
        embedding_model: Arc<TextEmbedding>,
        vector_size: u64,
    ) -> Self {
        Self {
            qdrant,
            embedding_model,
            vector_size,
        }
    }

    /// Index markdown documents from `docs_dir` into a language-specific Qdrant collection.
    ///
    /// Drops and recreates the collection (idempotent). Returns a summary of the operation.
    pub async fn index(&self, docs_dir: &Path, language: &str) -> Result<IndexSummary> {
        let collection = collection_name(language);

        // Read and chunk documents
        let chunks = chunking::read_and_chunk_docs(docs_dir)?;
        let files_processed = count_unique_files(&chunks);
        let total_chunks = chunks.len();

        info!(
            collection = %collection,
            files = files_processed,
            chunks = total_chunks,
            "indexing documents"
        );

        // Drop and recreate collection
        self.recreate_collection(&collection).await?;

        // Embed chunks in small batches to avoid ONNX runtime memory blowup.
        let texts: Vec<String> = chunks.iter().map(|c| c.text.clone()).collect();
        let mut embeddings = Vec::with_capacity(texts.len());
        for batch in texts.chunks(EMBED_BATCH_SIZE) {
            let batch_embeddings = self
                .embedding_model
                .embed(batch.to_vec(), None)
                .context("Failed to embed document chunks")?;
            embeddings.extend(batch_embeddings);
        }

        // Upsert in batches
        for batch_start in (0..chunks.len()).step_by(UPSERT_BATCH_SIZE) {
            let batch_end = (batch_start + UPSERT_BATCH_SIZE).min(chunks.len());
            let mut points = Vec::with_capacity(batch_end - batch_start);

            for i in batch_start..batch_end {
                let chunk = &chunks[i];
                let embedding = &embeddings[i];

                let mut payload = Payload::new();
                payload.insert("text", chunk.text.as_str());
                payload.insert("source_file", chunk.source_file.as_str());
                payload.insert("language", language);

                let point =
                    PointStruct::new(Uuid::new_v4().to_string(), embedding.clone(), payload);
                points.push(point);
            }

            self.qdrant
                .upsert_points(UpsertPointsBuilder::new(&collection, points).wait(true))
                .await
                .context("Failed to upsert points to Qdrant")?;

            debug!(batch_start, batch_end, "upserted batch to Qdrant");
        }

        info!(
            collection = %collection,
            files = files_processed,
            chunks = total_chunks,
            "indexing complete"
        );

        Ok(IndexSummary {
            files_processed: files_processed as u32,
            chunks_indexed: total_chunks as u32,
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
        let embeddings = self
            .embedding_model
            .embed(vec![query.to_string()], None)
            .context("Failed to embed search query")?;

        let query_vector = embeddings
            .into_iter()
            .next()
            .context("Embedding model returned no vectors")?;

        let response = self
            .qdrant
            .search_points(SearchPointsBuilder::new(&collection, query_vector, limit))
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
