use std::collections::HashMap;

use tonic::{Code, Request, Response, Status};
use tracing::info;

use remote_repo::{CreatePrOpts, GhBackend, MergeStrategy as BackendMergeStrategy, RemoteRepo};
use ur_rpc::error::{self, DOMAIN_REMOTE_REPO, INTERNAL, INVALID_ARGUMENT};
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
    pub builderd_addr: String,
}

impl RemoteRepoServiceHandler {
    fn backend(&self, gh_repo: &str) -> Result<GhBackend, RemoteRepoError> {
        if gh_repo.is_empty() {
            return Err(RemoteRepoError::InvalidArgument(
                "gh_repo must not be empty".into(),
            ));
        }
        Ok(GhBackend {
            builderd_addr: self.builderd_addr.clone(),
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
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "get_pr request");

        let backend = self.backend(&req.gh_repo)?;
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
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, title = %req.title, "create_pr request");

        let backend = self.backend(&req.gh_repo)?;
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
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "merge_pr request");

        let backend = self.backend(&req.gh_repo)?;
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
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "get_check_runs request");

        let backend = self.backend(&req.gh_repo)?;
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
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, run_id = req.run_id, "get_failed_run_logs request");

        let backend = self.backend(&req.gh_repo)?;
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
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "get_review_comments request");

        let backend = self.backend(&req.gh_repo)?;
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
        let req = req.into_inner();
        info!(gh_repo = %req.gh_repo, pr_number = req.pr_number, "get_conversation_comments request");

        let backend = self.backend(&req.gh_repo)?;
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
        let req = req.into_inner();
        info!(
            gh_repo = %req.gh_repo,
            pr_number = req.pr_number,
            comment_id = req.comment_id,
            "reply_to_comment request"
        );

        let backend = self.backend(&req.gh_repo)?;
        let comment_id = backend
            .reply_to_comment(req.pr_number, req.comment_id, &req.body)
            .await
            .map_err(|e| RemoteRepoError::Internal(e.to_string()))?;

        Ok(Response::new(ReplyToCommentResponse { comment_id }))
    }
}
