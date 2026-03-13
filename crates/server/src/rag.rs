use std::path::PathBuf;
use std::pin::Pin;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status};
use tracing::info;

use rag::RagManager;
use ur_rpc::proto::rag::rag_index_progress::Update;
use ur_rpc::proto::rag::rag_service_server::RagService;
use ur_rpc::proto::rag::{
    DependencyIndexed, IndexComplete, Language, RagIndexProgress, RagIndexRequest,
    RagSearchRequest, RagSearchResponse, RagSearchResult,
};

/// gRPC implementation of the RagService, delegating to `RagManager`.
#[derive(Clone)]
pub struct RagServiceHandler {
    pub rag_manager: RagManager,
    pub config_dir: PathBuf,
}

type RagIndexOutputStream =
    Pin<Box<dyn tokio_stream::Stream<Item = Result<RagIndexProgress, Status>> + Send>>;

#[tonic::async_trait]
impl RagService for RagServiceHandler {
    type RagIndexStream = RagIndexOutputStream;

    async fn rag_index(
        &self,
        req: Request<RagIndexRequest>,
    ) -> Result<Response<Self::RagIndexStream>, Status> {
        let req = req.into_inner();
        let language = language_str(req.language())?;

        info!(language = %language, "rag_index request received");

        let docs_dir = self.config_dir.join("rag/docs").join(language);

        if !docs_dir.exists() {
            return Err(Status::failed_precondition(format!(
                "docs directory does not exist: {}. Run `ur rag docs` first.",
                docs_dir.display()
            )));
        }

        let (progress_tx, mut progress_rx) = mpsc::channel::<rag::DependencyProgress>(32);
        let (stream_tx, stream_rx) = mpsc::channel::<Result<RagIndexProgress, Status>>(32);

        let rag_manager = self.rag_manager.clone();
        let lang_str = language.to_string();

        tokio::spawn(async move {
            // Forward dependency progress updates to the gRPC stream
            let stream_tx_clone = stream_tx.clone();
            let forward_handle = tokio::spawn(async move {
                while let Some(dep) = progress_rx.recv().await {
                    let msg = RagIndexProgress {
                        update: Some(Update::DependencyIndexed(DependencyIndexed {
                            name: dep.name,
                            files: dep.files,
                            chunks: dep.chunks,
                        })),
                    };
                    if stream_tx_clone.send(Ok(msg)).await.is_err() {
                        return;
                    }
                }
            });

            // Run the indexing
            let result = rag_manager
                .index(&docs_dir, &lang_str, Some(progress_tx))
                .await;

            // Wait for all progress messages to be forwarded
            let _ = forward_handle.await;

            // Send final message
            match result {
                Ok(summary) => {
                    let _ = stream_tx
                        .send(Ok(RagIndexProgress {
                            update: Some(Update::IndexComplete(IndexComplete {
                                total_files: summary.files_processed,
                                total_chunks: summary.chunks_indexed,
                            })),
                        }))
                        .await;
                }
                Err(e) => {
                    let _ = stream_tx
                        .send(Err(Status::internal(format!("indexing failed: {e}"))))
                        .await;
                }
            }
        });

        let stream = ReceiverStream::new(stream_rx);
        Ok(Response::new(Box::pin(stream) as Self::RagIndexStream))
    }

    async fn rag_search(
        &self,
        req: Request<RagSearchRequest>,
    ) -> Result<Response<RagSearchResponse>, Status> {
        let req = req.into_inner();
        let language = language_str(req.language())?;
        let top_k = req.top_k.map(|k| k as u64);

        info!(
            query = %req.query,
            language = %language,
            top_k = ?top_k,
            "rag_search request received"
        );

        let results = self
            .rag_manager
            .search(&req.query, language, top_k)
            .await
            .map_err(|e| Status::internal(format!("search failed: {e}")))?;

        let results = results
            .into_iter()
            .map(|r| RagSearchResult {
                text: r.text,
                source_file: r.source_file,
                score: r.score,
            })
            .collect();

        Ok(Response::new(RagSearchResponse { results }))
    }
}

/// Convert the proto `Language` enum to a string used by `RagManager`.
///
/// Returns `InvalidArgument` for `Unspecified` — callers must provide an explicit language.
#[allow(clippy::result_large_err)]
fn language_str(lang: Language) -> Result<&'static str, Status> {
    match lang {
        Language::Unspecified => Err(Status::invalid_argument(
            "language is required — specify a language (e.g. --language rust)",
        )),
        Language::Rust => Ok("rust"),
    }
}

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
