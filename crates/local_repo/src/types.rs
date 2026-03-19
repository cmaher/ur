/// Outcome of a git push operation.
#[derive(Debug, Clone)]
pub struct PushResult {
    pub status: PushStatus,
    /// The ref that was pushed (e.g. `refs/heads/main`).
    pub ref_name: String,
    /// Human-readable summary from git push output.
    pub summary: String,
}

/// Status of a git push operation, parsed from `git push --porcelain` output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushStatus {
    /// Push succeeded (fast-forward).
    Success,
    /// Push succeeded via forced update.
    ForcePushed,
    /// Push was rejected locally (non-fast-forward).
    Rejected { reason: String },
    /// Push was rejected by the remote (hook rejection, etc.).
    RemoteRejected { reason: String },
    /// Remote ref is already up to date.
    UpToDate,
    /// Pre-push hook failed (non-zero exit with no porcelain ref output).
    HookFailed { summary: String },
}

/// Outcome of running a git hook script.
#[derive(Debug, Clone)]
pub struct HookResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl HookResult {
    /// Returns true if the hook exited successfully (code 0).
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}
