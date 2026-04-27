use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::debug;
use ur_rpc::proto::builder::BuilderdClient;
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
        self.client
            .exec_collect("git", args, working_dir)
            .await
            .map_err(|e| anyhow!(e))
    }

    /// Execute a `git` command, check for success, and return stdout as a string.
    async fn exec_git_checked(&self, args: &[&str], working_dir: &str) -> Result<String> {
        let completed = self.exec_git(args, working_dir).await?;
        let completed = completed
            .check()
            .map_err(|e| anyhow!("git command failed: {e}"))?;
        Ok(completed.stdout_text())
    }

    /// Parse `git push --porcelain` output into a `PushResult`.
    ///
    /// Porcelain format per ref line: `<flag>\t<from>:<to>\t<summary> (<reason>)`
    /// Flags: ` ` = fast-forward, `+` = forced, `!` = rejected, `=` = up-to-date.
    fn parse_push_output(completed: &CompletedExec) -> PushResult {
        let stdout = completed.stdout_text();

        // Find the first ref status line (skip header lines like "To <url>")
        let ref_line = stdout.lines().find(|line| {
            // Porcelain ref lines start with a flag char followed by a tab
            line.len() >= 2 && line.as_bytes().get(1) == Some(&b'\t')
        });

        let Some(line) = ref_line else {
            // No parseable porcelain output — check exit code to distinguish
            // hook failures from successful pushes with unusual output.
            let fallback = if stdout.is_empty() {
                completed.stderr_text()
            } else {
                stdout
            };
            let summary = fallback.trim().to_string();
            let status = if completed.exit_code != 0 {
                PushStatus::HookFailed {
                    summary: summary.clone(),
                }
            } else {
                PushStatus::Success
            };
            return PushResult {
                status,
                ref_name: String::new(),
                summary,
            };
        };

        let flag = line.as_bytes()[0];

        // Split the rest after the first tab: <from>:<to>\t<summary>
        let after_flag = &line[2..]; // skip flag + tab
        let (refs_part, summary) = match after_flag.split_once('\t') {
            Some((r, s)) => (r, s.trim().to_string()),
            None => (after_flag, String::new()),
        };

        // Extract the destination ref (the "to" part of <from>:<to>)
        let ref_name = refs_part
            .split_once(':')
            .map(|(_, to)| to.to_string())
            .unwrap_or_else(|| refs_part.to_string());

        let status = match flag {
            b' ' => PushStatus::Success,
            b'+' => PushStatus::ForcePushed,
            b'=' => PushStatus::UpToDate,
            b'!' => {
                if summary.contains("remote rejected") {
                    PushStatus::RemoteRejected {
                        reason: summary.clone(),
                    }
                } else {
                    PushStatus::Rejected {
                        reason: summary.clone(),
                    }
                }
            }
            _ => PushStatus::Success,
        };

        PushResult {
            status,
            ref_name,
            summary,
        }
    }
}

#[async_trait]
impl LocalRepo for GitBackend {
    async fn current_branch(&self, working_dir: &str) -> Result<String> {
        let output = self
            .exec_git_checked(&["rev-parse", "--abbrev-ref", "HEAD"], working_dir)
            .await?;
        Ok(output.trim().to_string())
    }

    async fn push(&self, branch: &str, working_dir: &str, no_verify: bool) -> Result<PushResult> {
        let mut args = vec!["push", "--porcelain"];
        if no_verify {
            args.push("--no-verify");
        }
        args.extend_from_slice(&["-u", "origin", branch]);
        let completed = self.exec_git(&args, working_dir).await?;
        Ok(Self::parse_push_output(&completed))
    }

    async fn force_push(
        &self,
        branch: &str,
        working_dir: &str,
        no_verify: bool,
    ) -> Result<PushResult> {
        let mut args = vec!["push", "--porcelain", "--force-with-lease"];
        if no_verify {
            args.push("--no-verify");
        }
        args.extend_from_slice(&["-u", "origin", branch]);
        let completed = self.exec_git(&args, working_dir).await?;
        Ok(Self::parse_push_output(&completed))
    }

    async fn run_hook(&self, script_path: &str, working_dir: &str) -> Result<HookResult> {
        debug!(script_path = %script_path, working_dir = %working_dir, "executing hook via builderd");

        let completed = self
            .client
            .exec_collect(script_path, &[], working_dir)
            .await
            .map_err(|e| anyhow!(e))?;

        Ok(HookResult {
            exit_code: completed.exit_code,
            stdout: completed.stdout_text(),
            stderr: completed.stderr_text(),
        })
    }

    async fn clone(&self, url: &str, path: &str, parent_dir: &str) -> Result<()> {
        self.exec_git_checked(
            &[
                "clone",
                "--filter=blob:none",
                "--no-tags",
                "--single-branch",
                url,
                path,
            ],
            parent_dir,
        )
        .await?;
        Ok(())
    }

    async fn fetch(&self, working_dir: &str) -> Result<()> {
        self.exec_git_checked(&["fetch", "--no-tags", "origin"], working_dir)
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

    async fn checkout(&self, working_dir: &str, ref_name: &str) -> Result<()> {
        self.exec_git_checked(&["checkout", ref_name], working_dir)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn make_completed(stdout: &str, stderr: &str, exit_code: i32) -> CompletedExec {
        CompletedExec {
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
            exit_code,
        }
    }

    #[test]
    fn parse_push_success() {
        let completed = make_completed(
            "To github.com:user/repo.git\n \trefs/heads/main:refs/heads/main\t1234abc..5678def\n",
            "",
            0,
        );
        let result = GitBackend::parse_push_output(&completed);
        assert_eq!(result.status, PushStatus::Success);
        assert_eq!(result.ref_name, "refs/heads/main");
        assert_eq!(result.summary, "1234abc..5678def");
    }

    #[test]
    fn parse_push_force_pushed() {
        let completed = make_completed(
            "To github.com:user/repo.git\n+\trefs/heads/feature:refs/heads/feature\t1234abc...5678def (forced update)\n",
            "",
            0,
        );
        let result = GitBackend::parse_push_output(&completed);
        assert_eq!(result.status, PushStatus::ForcePushed);
        assert_eq!(result.ref_name, "refs/heads/feature");
        assert_eq!(result.summary, "1234abc...5678def (forced update)");
    }

    #[test]
    fn parse_push_up_to_date() {
        let completed = make_completed(
            "To github.com:user/repo.git\n=\trefs/heads/main:refs/heads/main\t[up to date]\n",
            "",
            0,
        );
        let result = GitBackend::parse_push_output(&completed);
        assert_eq!(result.status, PushStatus::UpToDate);
        assert_eq!(result.ref_name, "refs/heads/main");
        assert_eq!(result.summary, "[up to date]");
    }

    #[test]
    fn parse_push_rejected() {
        let completed = make_completed(
            "To github.com:user/repo.git\n!\trefs/heads/main:refs/heads/main\t[rejected] (non-fast-forward)\n",
            "",
            1,
        );
        let result = GitBackend::parse_push_output(&completed);
        assert_eq!(
            result.status,
            PushStatus::Rejected {
                reason: "[rejected] (non-fast-forward)".to_string()
            }
        );
        assert_eq!(result.ref_name, "refs/heads/main");
    }

    #[test]
    fn parse_push_remote_rejected() {
        let completed = make_completed(
            "To github.com:user/repo.git\n!\trefs/heads/main:refs/heads/main\t[remote rejected] (hook declined)\n",
            "",
            1,
        );
        let result = GitBackend::parse_push_output(&completed);
        assert_eq!(
            result.status,
            PushStatus::RemoteRejected {
                reason: "[remote rejected] (hook declined)".to_string()
            }
        );
        assert_eq!(result.ref_name, "refs/heads/main");
    }

    #[test]
    fn parse_push_empty_output_falls_back_to_stderr() {
        let completed = make_completed("", "Everything up-to-date", 0);
        let result = GitBackend::parse_push_output(&completed);
        assert_eq!(result.status, PushStatus::Success);
        assert_eq!(result.ref_name, "");
        assert_eq!(result.summary, "Everything up-to-date");
    }

    #[test]
    fn parse_push_empty_output_and_stderr() {
        let completed = make_completed("", "", 0);
        let result = GitBackend::parse_push_output(&completed);
        assert_eq!(result.status, PushStatus::Success);
        assert_eq!(result.ref_name, "");
        assert_eq!(result.summary, "");
    }

    #[test]
    fn parse_push_unexpected_format_falls_back() {
        let completed = make_completed("Some unexpected output\nwithout porcelain format\n", "", 0);
        let result = GitBackend::parse_push_output(&completed);
        // No porcelain line found, falls back to Success with raw output
        assert_eq!(result.status, PushStatus::Success);
        assert_eq!(result.ref_name, "");
    }

    #[test]
    fn parse_push_hook_failed_nonzero_exit() {
        let completed = make_completed(
            "",
            "error: failed to push some refs\npre-push hook declined",
            1,
        );
        let result = GitBackend::parse_push_output(&completed);
        assert!(
            matches!(result.status, PushStatus::HookFailed { .. }),
            "expected HookFailed, got {:?}",
            result.status
        );
        assert!(result.summary.contains("pre-push hook declined"));
    }

    #[test]
    fn parse_push_nonzero_exit_with_porcelain_uses_porcelain() {
        // If porcelain output exists, parse it normally even with non-zero exit
        let completed = make_completed(
            "To github.com:user/repo.git\n!\trefs/heads/main:refs/heads/main\t[rejected] (non-fast-forward)\n",
            "",
            1,
        );
        let result = GitBackend::parse_push_output(&completed);
        assert!(
            matches!(result.status, PushStatus::Rejected { .. }),
            "expected Rejected, got {:?}",
            result.status
        );
    }
}
