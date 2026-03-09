pub mod config;
pub mod credential;
pub mod git_exec;
pub mod grpc;
#[cfg(feature = "gh")]
pub mod grpc_gh;
#[cfg(feature = "git")]
pub mod grpc_git;
pub mod grpc_server;
pub mod process;
pub mod proxy;
pub mod stream;

pub use config::Config;
pub use credential::CredentialManager;
pub use git_exec::RepoRegistry;
pub use process::{ProcessConfig, ProcessManager};
pub use proxy::SquidManager;
/// Temporary compatibility alias — next ticket (ur-m5hk) migrates all callers.
pub use proxy::ProxyManager;
