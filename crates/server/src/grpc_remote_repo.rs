use std::collections::HashMap;

use tonic::{Code, Request, Response, Status};
use tracing::info;

use remote_repo::{CreatePrOpts, GhBackend, MergeStrategy as BackendMergeStrategy, RemoteRepo};
use ur_rpc::error::{self, DOMAIN_REMOTE_REPO, INTERNAL, INVALID_ARGUMENT};
use ur_rpc::proto::builder::BuilderdClient;
use ur_rpc::proto::remote_repo::remote_repo_service_server::RemoteRepoService;
use ur_rpc::proto::remote_repo::{
    CreatePrRequest, CreatePrResponse, GetCheckRunsRequest, GetCheckRunsResponse,
    GetConversationCommentsRequest, GetConversationCommentsResponse, GetFailedRunLogsRequest,
    GetFailedRunLogsResponse, GetPrRequest, GetPrResponse, GetReviewCommentsRequest,
    GetReviewCommentsResponse, MergePrRequest, MergePrResponse,
    MergeStrategy as ProtoMergeStrategy, ReplyToCommentRequest, ReplyToCommentResponse,
};

#[derive(Debug, thiserror::Error)]
pub enum RemoteRepoError {
    #[error("internal error: {0}")]
    Internal(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}

impl From<RemoteRepoError> for Status {
    fn from(err: RemoteRepoError) -> Self {
        match &err {
            RemoteRepoError::Internal(_) => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_REMOTE_REPO,
                INTERNAL,
                HashMap::new(),
            ),
            RemoteRepoError::InvalidArgument(_) => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_REMOTE_REPO,
                INVALID_ARGUMENT,
                HashMap::new(),
            ),
        }
    }
}

/// gRPC implementation of the RemoteRepoService, delegating to `GhBackend`.
#[derive(Clone)]
pub struct RemoteRepoServiceHandler {
    /// `gh` runs on the calling worker's machine (its gh auth). Served only
    /// on the worker port behind the auth interceptor, so every call names
    /// a worker.
    pub machine_registry: crate::MachineRegistryManager,
    /// Resolves the caller's `ur-worker-id` to its row's machine.
    pub worker_repo: workflow_db::WorkerRepo,
}

impl RemoteRepoServiceHandler {
    /// The builderd client of the caller's machine: its worker row's
    /// `machine_id` (without `ur-worker-id`, which the auth interceptor
    /// rules out on the server, the local machine). A machine the registry
    /// does not hold is `failed_precondition`.
    async fn builderd_for(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<BuilderdClient, Status> {
        let Some(header) = metadata.get(ur_config::WORKER_ID_HEADER) else {
            return Ok(self.machine_registry.local_builderd_client());
        };
        let worker_id = header.to_str().map_err(|_| {
            RemoteRepoError::InvalidArgument("invalid ur-worker-id header encoding".into())
        })?;
        let worker = self
            .worker_repo
            .get_worker(worker_id)
            .await
            .map_err(|e| RemoteRepoError::Internal(format!("db error: {e}")))?
            .ok_or_else(|| Status::not_found(format!("unknown worker: {worker_id}")))?;
        Ok(self.machine_registry.builderd_client(&worker.machine_id)?)
    }

    async fn backend<T>(&self, req: &Request<T>, gh_repo: &str) -> Result<GhBackend, Status> {
        if gh_repo.is_empty() {
            return Err(
                RemoteRepoError::InvalidArgument("gh_repo must not be empty".into()).into(),
            );
        }
        Ok(GhBackend {
            client: self.builderd_for(req.metadata()).await?,
            gh_repo: gh_repo.to_string(),
        })
    }
}

fn pr_to_proto(pr: remote_repo::PullRequest) -> ur_rpc::proto::remote_repo::PullRequest {
    ur_rpc::proto::remote_repo::PullRequest {
        number: pr.number,
        url: pr.url,
        state: pr.state,
        head_ref: pr.head_ref,
        base_ref: pr.base_ref,
        title: pr.title,
        body: pr.body,
    }
}

fn reactions_to_proto(r: remote_repo::Reactions) -> ur_rpc::proto::remote_repo::Reactions {
    ur_rpc::proto::remote_repo::Reactions {
        plus_one: r.plus_one,
        minus_one: r.minus_one,
        laugh: r.laugh,
        confused: r.confused,
        heart: r.heart,
        hooray: r.hooray,
        rocket: r.rocket,
        eyes: r.eyes,
    }
}

fn check_run_to_proto(c: remote_repo::CheckRun) -> ur_rpc::proto::remote_repo::CheckRun {
    ur_rpc::proto::remote_repo::CheckRun {
        name: c.name,
        status: c.status,
        conclusion: c.conclusion,
        details_url: c.details_url,
    }
}

fn review_comment_to_proto(
    c: remote_repo::ReviewComment,
) -> ur_rpc::proto::remote_repo::ReviewComment {
    ur_rpc::proto::remote_repo::ReviewComment {
        id: c.id,
        user: c.user,
        is_bot: c.is_bot,
        path: c.path,
        line: c.line,
        diff_hunk: c.diff_hunk,
        body: c.body,
        reactions: Some(reactions_to_proto(c.reactions)),
        in_reply_to_id: c.in_reply_to_id.unwrap_or(0),
        created_at: c.created_at,
    }
}

fn conversation_comment_to_proto(
    c: remote_repo::ConversationComment,
) -> ur_rpc::proto::remote_repo::ConversationComment {
    ur_rpc::proto::remote_repo::ConversationComment {
        id: c.id,
        user: c.user,
        is_bot: c.is_bot,
        body: c.body,
        reactions: Some(reactions_to_proto(c.reactions)),
        created_at: c.created_at,
    }
}

fn proto_merge_strategy(proto: i32) -> Result<BackendMergeStrategy, RemoteRepoError> {
    match ProtoMergeStrategy::try_from(proto) {
        Ok(ProtoMergeStrategy::Squash) => Ok(BackendMergeStrategy::Squash),
        Ok(ProtoMergeStrategy::Merge) => Ok(BackendMergeStrategy::Merge),
        Ok(ProtoMergeStrategy::Rebase) => Ok(BackendMergeStrategy::Rebase),
        Ok(ProtoMergeStrategy::Unspecified) => Ok(BackendMergeStrategy::Squash),
        Err(_) => Err(RemoteRepoError::InvalidArgument(format!(
            "unknown merge strategy: {proto}"
        ))),
    }
}

fn merge_result_to_proto(r: remote_repo::MergeResult) -> ur_rpc::proto::remote_repo::MergeResult {
    ur_rpc::proto::remote_repo::MergeResult {
        success: r.success,
        sha: r.sha,
        error_message: r.error_message,
    }
}

#[tonic::async_trait]
impl RemoteRepoService for RemoteRepoServiceHandler {
    async fn get_pr(&self, req: Request<GetPrRequest>) -> Result<Response<GetPrResponse>, Status> {
        let backend = self.backend(&req, &req.get_ref().gh_repo).await?;
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "get_pr request");

        let pr = backend
            .get_pr(req.pr_number)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(GetPrResponse {
            pull_request: Some(pr_to_proto(pr)),
        }))
    }

    async fn create_pr(
        &self,
        req: Request<CreatePrRequest>,
    ) -> Result<Response<CreatePrResponse>, Status> {
        let backend = self.backend(&req, &req.get_ref().gh_repo).await?;
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, title = %req.title, "create_pr request");

        let opts = CreatePrOpts {
            title: req.title,
            body: req.body,
            head: req.head,
            base: req.base,
            draft: req.draft,
        };
        let pr = backend
            .create_pr(opts)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(CreatePrResponse {
            pull_request: Some(pr_to_proto(pr)),
        }))
    }

    async fn merge_pr(
        &self,
        req: Request<MergePrRequest>,
    ) -> Result<Response<MergePrResponse>, Status> {
        let backend = self.backend(&req, &req.get_ref().gh_repo).await?;
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "merge_pr request");

        let strategy = proto_merge_strategy(req.strategy)?;
        let result = backend
            .merge_pr(req.pr_number, strategy)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(MergePrResponse {
            result: Some(merge_result_to_proto(result)),
        }))
    }

    async fn get_check_runs(
        &self,
        req: Request<GetCheckRunsRequest>,
    ) -> Result<Response<GetCheckRunsResponse>, Status> {
        let backend = self.backend(&req, &req.get_ref().gh_repo).await?;
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "get_check_runs request");

        let runs = backend
            .check_runs(req.pr_number)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(GetCheckRunsResponse {
            check_runs: runs.into_iter().map(check_run_to_proto).collect(),
        }))
    }

    async fn get_failed_run_logs(
        &self,
        req: Request<GetFailedRunLogsRequest>,
    ) -> Result<Response<GetFailedRunLogsResponse>, Status> {
        let backend = self.backend(&req, &req.get_ref().gh_repo).await?;
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, run_id = req.run_id, "get_failed_run_logs request");

        let logs = backend
            .failed_run_logs(req.run_id)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(GetFailedRunLogsResponse { logs }))
    }

    async fn get_review_comments(
        &self,
        req: Request<GetReviewCommentsRequest>,
    ) -> Result<Response<GetReviewCommentsResponse>, Status> {
        let backend = self.backend(&req, &req.get_ref().gh_repo).await?;
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "get_review_comments request");

        let comments = backend
            .get_review_comments(req.pr_number)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(GetReviewCommentsResponse {
            comments: comments.into_iter().map(review_comment_to_proto).collect(),
        }))
    }

    async fn get_conversation_comments(
        &self,
        req: Request<GetConversationCommentsRequest>,
    ) -> Result<Response<GetConversationCommentsResponse>, Status> {
        let backend = self.backend(&req, &req.get_ref().gh_repo).await?;
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "get_conversation_comments request");

        let comments = backend
            .get_conversation_comments(req.pr_number)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(GetConversationCommentsResponse {
            comments: comments
                .into_iter()
                .map(conversation_comment_to_proto)
                .collect(),
        }))
    }

    async fn reply_to_comment(
        &self,
        req: Request<ReplyToCommentRequest>,
    ) -> Result<Response<ReplyToCommentResponse>, Status> {
        let backend = self.backend(&req, &req.get_ref().gh_repo).await?;
        let req = req.into_inner();
        info!(
            gh_repo = %req.gh_repo,
            pr_number = req.pr_number,
            comment_id = req.comment_id,
            "reply_to_comment request"
        );

        let comment_id = backend
            .reply_to_comment(req.pr_number, req.comment_id, &req.body)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(ReplyToCommentResponse { comment_id }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker_row(worker_id: &str, machine_id: &str) -> workflow_db::model::Worker {
        let now = chrono::Utc::now().to_rfc3339();
        workflow_db::model::Worker {
            worker_id: worker_id.into(),
            process_id: worker_id.into(),
            project_key: "proj".into(),
            container_id: String::new(),
            worker_secret: "secret".into(),
            strategy: "code".into(),
            agent_type: "claude".into(),
            container_status: "running".into(),
            agent_status: "idle".into(),
            workspace_path: None,
            created_at: now.clone(),
            updated_at: now,
            idle_redispatch_count: 0,
            brain_path: None,
            runtime: workflow_db::model::WorkerRuntime::Container,
            machine_id: machine_id.into(),
        }
    }

    fn metadata(worker_id: Option<&str>) -> tonic::metadata::MetadataMap {
        let mut m = tonic::metadata::MetadataMap::new();
        if let Some(id) = worker_id {
            m.insert(ur_config::WORKER_ID_HEADER, id.parse().unwrap());
        }
        m
    }

    /// A worker's call resolves to its row's machine; one on a machine the
    /// registry does not hold is `failed_precondition` (never local); no
    /// worker is the local machine; an unknown worker is `not_found`.
    #[tokio::test]
    async fn builderd_follows_the_calling_workers_machine() {
        let db = ur_db_test::TestDb::new().await;
        let worker_repo = workflow_db::WorkerRepo::new(db.workflow_pool().clone());
        worker_repo
            .insert_worker(&worker_row("w-local", "mbp"))
            .await
            .unwrap();
        worker_repo
            .insert_worker(&worker_row("w-away", "elsewhere"))
            .await
            .unwrap();
        let handler = RemoteRepoServiceHandler {
            machine_registry: crate::MachineRegistryManager::new(
                "mbp".into(),
                "http://127.0.0.1:1".into(),
            )
            .unwrap(),
            worker_repo,
        };

        assert!(handler.builderd_for(&metadata(None)).await.is_ok());
        assert!(
            handler
                .builderd_for(&metadata(Some("w-local")))
                .await
                .is_ok()
        );
        let status = handler
            .builderd_for(&metadata(Some("w-away")))
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::FailedPrecondition, "{status:?}");
        assert!(status.message().contains("elsewhere"), "{status:?}");
        let status = handler
            .builderd_for(&metadata(Some("w-gone")))
            .await
            .unwrap_err();
        assert_eq!(status.code(), Code::NotFound, "{status:?}");
    }
}
