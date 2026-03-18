use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use tracing::debug;
use ur_rpc::proto::builder::BuilderExecMessage;
use ur_rpc::proto::builder::BuilderExecRequest;
use ur_rpc::proto::builder::builder_daemon_service_client::BuilderDaemonServiceClient;
use ur_rpc::proto::builder::builder_exec_message::Payload as ExecPayload;
use ur_rpc::stream::CompletedExec;

use crate::r#trait::RemoteRepo;
use crate::types::{
    CheckRun, ConversationComment, CreatePrOpts, MergeResult, MergeStrategy, PullRequest,
    Reactions, ReviewComment,
};

/// Implements `RemoteRepo` by routing `gh` CLI commands through a builderd daemon.
#[derive(Clone)]
pub struct GhBackend {
    pub client: BuilderDaemonServiceClient<tonic::transport::Channel>,
    pub gh_repo: String,
}

impl GhBackend {
    /// Execute a `gh` command via builderd and return the completed execution.
    async fn exec_gh(&self, args: &[&str]) -> Result<CompletedExec> {
        debug!(repo = %self.gh_repo, args = ?args, "executing gh command via builderd");

        let mut client = self.client.clone();

        let req = BuilderExecRequest {
            command: "gh".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            working_dir: "/tmp".into(),
            env: std::collections::HashMap::new(),
            long_lived: false,
        };

        let start_msg = BuilderExecMessage {
            payload: Some(ExecPayload::Start(req)),
        };

        let response = client
            .exec(tokio_stream::once(start_msg))
            .await
            .context("builderd exec failed")?;

        let stream = response.into_inner();
        let completed = CompletedExec::collect(stream)
            .await
            .map_err(|e| anyhow!("stream error: {e}"))?;

        Ok(completed)
    }

    /// Execute a `gh` command, check for success, and return stdout as a string.
    async fn exec_gh_checked(&self, args: &[&str]) -> Result<String> {
        let completed = self.exec_gh(args).await?;
        let completed = completed
            .check()
            .map_err(|e| anyhow!("gh command failed: {e}"))?;
        Ok(completed.stdout_text())
    }

    /// Execute a `gh` command, check for success, and parse stdout as JSON.
    async fn exec_gh_json<T: serde::de::DeserializeOwned>(&self, args: &[&str]) -> Result<T> {
        let text = self.exec_gh_checked(args).await?;
        serde_json::from_str(&text).context("failed to parse gh JSON output")
    }

    fn parse_pr_from_api(value: &serde_json::Value) -> Result<PullRequest> {
        Ok(PullRequest {
            number: value["number"].as_i64().unwrap_or(0),
            url: value["html_url"]
                .as_str()
                .or_else(|| value["url"].as_str())
                .unwrap_or("")
                .to_string(),
            state: value["state"].as_str().unwrap_or("").to_string(),
            head_ref: value["head"]["ref"]
                .as_str()
                .or_else(|| value["headRefName"].as_str())
                .unwrap_or("")
                .to_string(),
            base_ref: value["base"]["ref"]
                .as_str()
                .or_else(|| value["baseRefName"].as_str())
                .unwrap_or("")
                .to_string(),
            title: value["title"].as_str().unwrap_or("").to_string(),
            body: value["body"].as_str().unwrap_or("").to_string(),
        })
    }

    fn parse_reactions(value: &serde_json::Value) -> Reactions {
        Reactions {
            plus_one: value["+1"].as_i64().unwrap_or(0),
            minus_one: value["-1"].as_i64().unwrap_or(0),
            laugh: value["laugh"].as_i64().unwrap_or(0),
            confused: value["confused"].as_i64().unwrap_or(0),
            heart: value["heart"].as_i64().unwrap_or(0),
            hooray: value["hooray"].as_i64().unwrap_or(0),
            rocket: value["rocket"].as_i64().unwrap_or(0),
            eyes: value["eyes"].as_i64().unwrap_or(0),
        }
    }
}

#[async_trait]
impl RemoteRepo for GhBackend {
    async fn get_pr(&self, pr_number: i64) -> Result<PullRequest> {
        let endpoint = format!("repos/{}/pulls/{}", self.gh_repo, pr_number);
        let value: serde_json::Value = self.exec_gh_json(&["api", &endpoint]).await?;
        Self::parse_pr_from_api(&value)
    }

    async fn create_pr(&self, opts: CreatePrOpts) -> Result<PullRequest> {
        let mut args = vec![
            "pr",
            "create",
            "--repo",
            &self.gh_repo,
            "--title",
            &opts.title,
            "--body",
            &opts.body,
            "--head",
            &opts.head,
            "--base",
            &opts.base,
        ];
        if opts.draft {
            args.push("--draft");
        }
        args.extend_from_slice(&[
            "--json",
            "number,url,state,headRefName,baseRefName,title,body",
        ]);

        let value: serde_json::Value = self.exec_gh_json(&args).await?;
        Self::parse_pr_from_api(&value)
    }

    async fn merge_pr(&self, pr_number: i64, strategy: MergeStrategy) -> Result<MergeResult> {
        let pr_str = pr_number.to_string();
        let strategy_flag = match strategy {
            MergeStrategy::Squash => "--squash",
            MergeStrategy::Merge => "--merge",
            MergeStrategy::Rebase => "--rebase",
        };

        let completed = self
            .exec_gh(&[
                "pr",
                "merge",
                &pr_str,
                "--repo",
                &self.gh_repo,
                strategy_flag,
            ])
            .await?;

        let stderr_text = completed.stderr_text();
        let stdout_text = completed.stdout_text();

        if completed.exit_code != 0 {
            let has_conflict = stderr_text.contains("merge conflict")
                || stderr_text.contains("not mergeable")
                || stderr_text.contains("conflicts");
            return Ok(MergeResult {
                success: false,
                sha: String::new(),
                error_message: if has_conflict {
                    format!("merge conflict: {stderr_text}")
                } else {
                    stderr_text
                },
            });
        }

        // Try to extract the merge SHA from stdout — gh may print it
        let sha = stdout_text
            .lines()
            .find_map(|line| {
                // gh pr merge often outputs something like "Merged via ..."
                // The SHA might not always be present; return empty if not found
                if line.contains("sha") || line.len() == 40 {
                    Some(line.trim().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        Ok(MergeResult {
            success: true,
            sha,
            error_message: String::new(),
        })
    }

    async fn check_runs(&self, pr_number: i64) -> Result<Vec<CheckRun>> {
        let pr_str = pr_number.to_string();
        let value: Vec<serde_json::Value> = self
            .exec_gh_json(&[
                "pr",
                "checks",
                &pr_str,
                "--repo",
                &self.gh_repo,
                "--json",
                "name,state,conclusion,detailsUrl",
            ])
            .await?;

        let runs = value
            .iter()
            .map(|v| CheckRun {
                name: v["name"].as_str().unwrap_or("").to_string(),
                status: v["state"].as_str().unwrap_or("").to_string(),
                conclusion: v["conclusion"].as_str().unwrap_or("").to_string(),
                details_url: v["detailsUrl"].as_str().unwrap_or("").to_string(),
            })
            .collect();

        Ok(runs)
    }

    async fn failed_run_logs(&self, run_id: i64) -> Result<String> {
        let run_str = run_id.to_string();
        self.exec_gh_checked(&[
            "run",
            "view",
            &run_str,
            "--repo",
            &self.gh_repo,
            "--log-failed",
        ])
        .await
    }

    async fn get_review_comments(&self, pr_number: i64) -> Result<Vec<ReviewComment>> {
        let endpoint = format!(
            "repos/{}/pulls/{}/comments?per_page=100",
            self.gh_repo, pr_number
        );
        let values: Vec<serde_json::Value> = self.exec_gh_json(&["api", &endpoint]).await?;

        let comments = values
            .iter()
            .map(|v| ReviewComment {
                id: v["id"].as_i64().unwrap_or(0),
                user: v["user"]["login"].as_str().unwrap_or("").to_string(),
                is_bot: v["user"]["type"].as_str().unwrap_or("") == "Bot",
                path: v["path"].as_str().unwrap_or("").to_string(),
                line: v["line"].as_i64().unwrap_or(0),
                diff_hunk: v["diff_hunk"].as_str().unwrap_or("").to_string(),
                body: v["body"].as_str().unwrap_or("").to_string(),
                reactions: Self::parse_reactions(&v["reactions"]),
                in_reply_to_id: v["in_reply_to_id"].as_i64(),
                created_at: v["created_at"].as_str().unwrap_or("").to_string(),
            })
            .collect();

        Ok(comments)
    }

    async fn get_conversation_comments(&self, pr_number: i64) -> Result<Vec<ConversationComment>> {
        let endpoint = format!(
            "repos/{}/issues/{}/comments?per_page=100",
            self.gh_repo, pr_number
        );
        let values: Vec<serde_json::Value> = self.exec_gh_json(&["api", &endpoint]).await?;

        let comments = values
            .iter()
            .map(|v| ConversationComment {
                id: v["id"].as_i64().unwrap_or(0),
                user: v["user"]["login"].as_str().unwrap_or("").to_string(),
                is_bot: v["user"]["type"].as_str().unwrap_or("") == "Bot",
                body: v["body"].as_str().unwrap_or("").to_string(),
                reactions: Self::parse_reactions(&v["reactions"]),
                created_at: v["created_at"].as_str().unwrap_or("").to_string(),
            })
            .collect();

        Ok(comments)
    }

    async fn reply_to_comment(&self, pr_number: i64, comment_id: i64, body: &str) -> Result<i64> {
        let endpoint = format!(
            "repos/{}/pulls/{}/comments/{}/replies",
            self.gh_repo, pr_number, comment_id
        );
        let value: serde_json::Value = self
            .exec_gh_json(&["api", &endpoint, "-f", &format!("body={body}")])
            .await?;

        value["id"]
            .as_i64()
            .ok_or_else(|| anyhow!("reply response missing id"))
    }
}
