use std::path::PathBuf;

use tonic::{Request, Response, Status};
use tracing::info;

use rag::RagManager;
use ur_rpc::proto::rag::rag_service_server::RagService;
use ur_rpc::proto::rag::{
    Language, RagIndexRequest, RagIndexResponse, RagSearchRequest, RagSearchResponse,
    RagSearchResult,
};

/// gRPC implementation of the RagService, delegating to `RagManager`.
#[derive(Clone)]
pub struct RagServiceHandler {
    pub rag_manager: RagManager,
    pub config_dir: PathBuf,
}

#[tonic::async_trait]
impl RagService for RagServiceHandler {
    async fn rag_index(
        &self,
        req: Request<RagIndexRequest>,
    ) -> Result<Response<RagIndexResponse>, Status> {
        let req = req.into_inner();
        let language = language_str(req.language());

        info!(language = %language, "rag_index request received");

        let docs_dir = self.config_dir.join("rag/docs").join(language);

        if !docs_dir.exists() {
            return Err(Status::failed_precondition(format!(
                "docs directory does not exist: {}. Run `ur rag docs` first.",
                docs_dir.display()
            )));
        }

        let summary = self
            .rag_manager
            .index(&docs_dir, language)
            .await
            .map_err(|e| Status::internal(format!("indexing failed: {e}")))?;

        Ok(Response::new(RagIndexResponse {
            files_processed: summary.files_processed,
            chunks_indexed: summary.chunks_indexed,
        }))
    }

    async fn rag_search(
        &self,
        req: Request<RagSearchRequest>,
    ) -> Result<Response<RagSearchResponse>, Status> {
        let req = req.into_inner();
        let language = language_str(req.language());
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
fn language_str(lang: Language) -> &'static str {
    match lang {
        Language::Rust => "rust",
    }
}
