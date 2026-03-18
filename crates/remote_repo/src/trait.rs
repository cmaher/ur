use anyhow::Result;
use async_trait::async_trait;

use crate::types::{
    CheckRun, ConversationComment, CreatePrOpts, MergeResult, MergeStrategy, PullRequest,
    ReviewComment,
};

#[async_trait]
pub trait RemoteRepo: Send + Sync {
    async fn get_pr(&self, pr_number: i64) -> Result<PullRequest>;
    async fn create_pr(&self, opts: CreatePrOpts) -> Result<PullRequest>;
    async fn merge_pr(&self, pr_number: i64, strategy: MergeStrategy) -> Result<MergeResult>;
    async fn check_runs(&self, pr_number: i64) -> Result<Vec<CheckRun>>;
    async fn failed_run_logs(&self, run_id: i64) -> Result<String>;
    async fn get_review_comments(&self, pr_number: i64) -> Result<Vec<ReviewComment>>;
    async fn get_conversation_comments(&self, pr_number: i64) -> Result<Vec<ConversationComment>>;
    async fn reply_to_comment(&self, pr_number: i64, comment_id: i64, body: &str) -> Result<i64>;

    /// Reply to a comment with a bot prefix.
    async fn reply_bot_comment(&self, pr_number: i64, comment_id: i64, body: &str) -> Result<i64> {
        let prefixed = format!("\u{1F916} {body}");
        self.reply_to_comment(pr_number, comment_id, &prefixed)
            .await
    }
}
