use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use tracing::debug;
use ur_rpc::proto::builder::{
    BuilderExecMessage, BuilderExecRequest, BuilderdClient,
    builder_exec_message::Payload as ExecPayload,
};
use ur_rpc::stream::CompletedExec;

use crate::r#trait::LocalRepo;
use crate::types::{HookResult, PushResult, PushStatus};

/// Implements `LocalRepo` by routing `git` CLI commands through a builderd daemon.
#[derive(Clone)]
pub struct GitBackend {
    pub client: BuilderdClient,
}

impl GitBackend {
    /// Execute a `git` command via builderd and return the completed execution.
    async fn exec_git(&self, args: &[&str], working_dir: &str) -> Result<CompletedExec> {
        debug!(args = ?args, working_dir = %working_dir, "executing git command via builderd");

        let mut client = self.client.clone();

        let req = BuilderExecRequest {
            command: "git".into(),
            args: args.iter().map(|s| s.to_string()).collect(),
            working_dir: working_dir.into(),
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

    /// Execute a `git` command, check for success, and return stdout as a string.
    async fn exec_git_checked(&self, args: &[&str], working_dir: &str) -> Result<String> {
        let completed = self.exec_git(args, working_dir).await?;
        let completed = completed
            .check()
            .map_err(|e| anyhow!("git command failed: {e}"))?;
        Ok(completed.stdout_text())
    }

    /// Parse the output of `git push` into a `PushResult`.
    fn parse_push_output(completed: &CompletedExec) -> PushResult {
        let stderr = completed.stderr_text();
        let stdout = completed.stdout_text();

        if completed.exit_code == 0 {
            PushResult {
                status: PushStatus::Success,
                message: if stderr.is_empty() { stdout } else { stderr },
            }
        } else {
            // git push writes most output to stderr
            let output = if stderr.is_empty() { stdout } else { stderr };

            let status = if output.contains("rejected")
                || output.contains("non-fast-forward")
                || output.contains("[remote rejected]")
                || output.contains("hook declined")
            {
                PushStatus::Rejected
            } else {
                PushStatus::Error
            };

            PushResult {
                status,
                message: output,
            }
        }
    }
}

#[async_trait]
impl LocalRepo for GitBackend {
    async fn push(&self, branch: &str, working_dir: &str) -> Result<PushResult> {
        let completed = self
            .exec_git(&["push", "origin", branch], working_dir)
            .await?;
        Ok(Self::parse_push_output(&completed))
    }

    async fn force_push(&self, branch: &str, working_dir: &str) -> Result<PushResult> {
        let completed = self
            .exec_git(
                &["push", "--force-with-lease", "origin", branch],
                working_dir,
            )
            .await?;
        Ok(Self::parse_push_output(&completed))
    }

    async fn run_hook(&self, script_path: &str, working_dir: &str) -> Result<HookResult> {
        debug!(script_path = %script_path, working_dir = %working_dir, "executing hook via builderd");

        let mut client = self.client.clone();

        let req = BuilderExecRequest {
            command: script_path.into(),
            args: vec![],
            working_dir: working_dir.into(),
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

        Ok(HookResult {
            exit_code: completed.exit_code,
            stdout: completed.stdout_text(),
            stderr: completed.stderr_text(),
        })
    }

    async fn clone(&self, url: &str, path: &str, parent_dir: &str) -> Result<()> {
        self.exec_git_checked(&["clone", url, path], parent_dir)
            .await?;
        Ok(())
    }

    async fn fetch(&self, working_dir: &str) -> Result<()> {
        self.exec_git_checked(&["fetch", "origin"], working_dir)
            .await?;
        Ok(())
    }

    async fn reset_hard(&self, working_dir: &str, ref_name: &str) -> Result<()> {
        self.exec_git_checked(&["reset", "--hard", ref_name], working_dir)
            .await?;
        Ok(())
    }

    async fn clean(&self, working_dir: &str) -> Result<()> {
        self.exec_git_checked(&["clean", "-fdx"], working_dir)
            .await?;
        Ok(())
    }

    async fn checkout_branch(&self, working_dir: &str, branch: &str) -> Result<()> {
        self.exec_git_checked(&["checkout", "-B", branch], working_dir)
            .await?;
        Ok(())
    }

    async fn submodule_update(&self, working_dir: &str) -> Result<()> {
        self.exec_git_checked(
            &["submodule", "update", "--init", "--recursive"],
            working_dir,
        )
        .await?;
        Ok(())
    }
}
