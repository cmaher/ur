use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: i64,
    pub url: String,
    pub state: String,
    pub head_ref: String,
    pub base_ref: String,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatePrOpts {
    pub title: String,
    pub body: String,
    pub head: String,
    pub base: String,
    pub draft: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    pub success: bool,
    pub sha: String,
    pub error_message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MergeStrategy {
    Squash,
    Merge,
    Rebase,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckRun {
    pub name: String,
    pub status: String,
    pub conclusion: String,
    pub details_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: i64,
    pub user: String,
    pub is_bot: bool,
    pub path: String,
    pub line: i64,
    pub diff_hunk: String,
    pub body: String,
    pub reactions: Reactions,
    pub in_reply_to_id: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationComment {
    pub id: i64,
    pub user: String,
    pub is_bot: bool,
    pub body: String,
    pub reactions: Reactions,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reactions {
    pub plus_one: i64,
    pub minus_one: i64,
    pub laugh: i64,
    pub confused: i64,
    pub heart: i64,
    pub hooray: i64,
    pub rocket: i64,
    pub eyes: i64,
}
