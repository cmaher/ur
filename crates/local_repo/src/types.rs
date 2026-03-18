/// Outcome of a git push operation.
#[derive(Debug, Clone)]
pub struct PushResult {
    pub status: PushStatus,
    /// Human-readable summary from git push output.
    pub message: String,
}

/// Status of a git push operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PushStatus {
    /// Push succeeded (fast-forward or forced update).
    Success,
    /// Push was rejected (non-fast-forward, hook rejection, etc.).
    Rejected,
    /// Push encountered an error (network, auth, etc.).
    Error,
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
