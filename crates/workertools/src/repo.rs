use clap::Subcommand;
use tonic::transport::Channel;

use ur_rpc::proto::remote_repo::remote_repo_service_client::RemoteRepoServiceClient;
use ur_rpc::proto::remote_repo::{
    CreatePrRequest, GetCheckRunsRequest, GetConversationCommentsRequest, GetFailedRunLogsRequest,
    GetPrRequest, GetReviewCommentsRequest, ReplyToCommentRequest,
};

use crate::inject_auth;

#[derive(Subcommand)]
pub enum RepoCommands {
    /// Pull request operations
    Pr {
        #[command(subcommand)]
        command: PrCommands,
    },
    /// Comment operations
    Comments {
        #[command(subcommand)]
        command: CommentsCommands,
    },
    /// CI run operations
    Run {
        #[command(subcommand)]
        command: RunCommands,
    },
}

#[derive(Subcommand)]
pub enum PrCommands {
    /// Get pull request details
    Get {
        /// PR number
        pr_number: i64,
    },
    /// Create a new pull request
    Create {
        /// PR title
        #[arg(long)]
        title: String,
        /// PR body/description
        #[arg(long, default_value = "")]
        body: String,
        /// Head branch name
        #[arg(long)]
        head: String,
        /// Base branch name (defaults to repo default branch)
        #[arg(long, default_value = "")]
        base: String,
        /// Create as draft PR
        #[arg(long)]
        draft: bool,
    },
    /// Get check runs for a pull request
    Checks {
        /// PR number
        pr_number: i64,
    },
}

#[derive(Subcommand)]
pub enum CommentsCommands {
    /// Get review comments on a pull request
    Review {
        /// PR number
        pr_number: i64,
    },
    /// Get conversation comments on a pull request
    Conversation {
        /// PR number
        pr_number: i64,
    },
    /// Reply to a comment
    Reply {
        /// PR number the comment belongs to
        #[arg(long)]
        pr: i64,
        /// Comment ID to reply to
        comment_id: i64,
        /// Reply message
        message: String,
    },
}

#[derive(Subcommand)]
pub enum RunCommands {
    /// Get logs for a failed CI run
    Logs {
        /// Run ID
        run_id: i64,
    },
}

fn get_gh_repo() -> Result<String, String> {
    std::env::var("UR_GH_REPO")
        .map_err(|_| "UR_GH_REPO environment variable is not set. Workers must have project context to use repo commands.".to_owned())
}

async fn connect() -> Result<RemoteRepoServiceClient<Channel>, i32> {
    let server_addr =
        std::env::var(ur_config::UR_SERVER_ADDR_ENV).expect("UR_SERVER_ADDR must be set");
    let addr = format!("http://{server_addr}");

    let channel = match tonic::transport::Endpoint::try_from(addr)
        .unwrap()
        .connect()
        .await
    {
        Ok(ch) => ch,
        Err(e) => {
            eprintln!("repo: failed to connect to ur server: {e}");
            return Err(1);
        }
    };

    Ok(RemoteRepoServiceClient::new(channel))
}

fn print_json<T: serde::Serialize>(value: &T) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).expect("failed to serialize response")
    );
}

pub async fn run(command: RepoCommands) -> i32 {
    let gh_repo = match get_gh_repo() {
        Ok(r) => r,
        Err(msg) => {
            eprintln!("repo: {msg}");
            return 1;
        }
    };

    let mut client = match connect().await {
        Ok(c) => c,
        Err(code) => return code,
    };

    match command {
        RepoCommands::Pr { command } => run_pr(command, &mut client, &gh_repo).await,
        RepoCommands::Comments { command } => run_comments(command, &mut client, &gh_repo).await,
        RepoCommands::Run { command } => run_run(command, &mut client, &gh_repo).await,
    }
}

async fn run_pr(
    command: PrCommands,
    client: &mut RemoteRepoServiceClient<Channel>,
    gh_repo: &str,
) -> i32 {
    match command {
        PrCommands::Get { pr_number } => {
            let mut request = tonic::Request::new(GetPrRequest {
                gh_repo: gh_repo.to_owned(),
                pr_number,
            });
            inject_auth(&mut request);

            match client.get_pr(request).await {
                Ok(resp) => {
                    print_json(&resp.into_inner());
                    0
                }
                Err(status) => {
                    eprintln!("repo pr get: {}", status.message());
                    1
                }
            }
        }
        PrCommands::Create {
            title,
            body,
            head,
            base,
            draft,
        } => {
            let mut request = tonic::Request::new(CreatePrRequest {
                gh_repo: gh_repo.to_owned(),
                title,
                body,
                head,
                base,
                draft,
            });
            inject_auth(&mut request);

            match client.create_pr(request).await {
                Ok(resp) => {
                    print_json(&resp.into_inner());
                    0
                }
                Err(status) => {
                    eprintln!("repo pr create: {}", status.message());
                    1
                }
            }
        }
        PrCommands::Checks { pr_number } => {
            let mut request = tonic::Request::new(GetCheckRunsRequest {
                gh_repo: gh_repo.to_owned(),
                pr_number,
            });
            inject_auth(&mut request);

            match client.get_check_runs(request).await {
                Ok(resp) => {
                    print_json(&resp.into_inner());
                    0
                }
                Err(status) => {
                    eprintln!("repo pr checks: {}", status.message());
                    1
                }
            }
        }
    }
}

async fn run_comments(
    command: CommentsCommands,
    client: &mut RemoteRepoServiceClient<Channel>,
    gh_repo: &str,
) -> i32 {
    match command {
        CommentsCommands::Review { pr_number } => {
            let mut request = tonic::Request::new(GetReviewCommentsRequest {
                gh_repo: gh_repo.to_owned(),
                pr_number,
            });
            inject_auth(&mut request);

            match client.get_review_comments(request).await {
                Ok(resp) => {
                    print_json(&resp.into_inner());
                    0
                }
                Err(status) => {
                    eprintln!("repo comments review: {}", status.message());
                    1
                }
            }
        }
        CommentsCommands::Conversation { pr_number } => {
            let mut request = tonic::Request::new(GetConversationCommentsRequest {
                gh_repo: gh_repo.to_owned(),
                pr_number,
            });
            inject_auth(&mut request);

            match client.get_conversation_comments(request).await {
                Ok(resp) => {
                    print_json(&resp.into_inner());
                    0
                }
                Err(status) => {
                    eprintln!("repo comments conversation: {}", status.message());
                    1
                }
            }
        }
        CommentsCommands::Reply {
            pr,
            comment_id,
            message,
        } => {
            let mut request = tonic::Request::new(ReplyToCommentRequest {
                gh_repo: gh_repo.to_owned(),
                pr_number: pr,
                comment_id,
                body: message,
            });
            inject_auth(&mut request);

            match client.reply_to_comment(request).await {
                Ok(resp) => {
                    print_json(&resp.into_inner());
                    0
                }
                Err(status) => {
                    eprintln!("repo comments reply: {}", status.message());
                    1
                }
            }
        }
    }
}

async fn run_run(
    command: RunCommands,
    client: &mut RemoteRepoServiceClient<Channel>,
    gh_repo: &str,
) -> i32 {
    match command {
        RunCommands::Logs { run_id } => {
            let mut request = tonic::Request::new(GetFailedRunLogsRequest {
                gh_repo: gh_repo.to_owned(),
                run_id,
            });
            inject_auth(&mut request);

            match client.get_failed_run_logs(request).await {
                Ok(resp) => {
                    print_json(&resp.into_inner());
                    0
                }
                Err(status) => {
                    eprintln!("repo run logs: {}", status.message());
                    1
                }
            }
        }
    }
}
