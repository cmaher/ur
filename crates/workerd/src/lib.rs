//! Worker daemon library.
//!
//! Hosts the `WorkerDaemonService` gRPC implementation, the agent exit
//! watcher, and the init managers. The container `workerd` binary wires these
//! with [`WorkerPaths::container`], [`WorkerInitInputs::from_env`] and a
//! [`ContainerExitSink`]; a host-side runtime can wire them with its own
//! paths, per-worker inputs, tmux session and exit sink.

pub mod container_exit;
pub mod dispatch;
pub mod exit_watcher;
pub mod grpc_service;
pub mod init;
pub mod init_git_hooks;
pub mod init_skill_hooks;
pub mod paths;
pub mod skill_layout;

#[cfg(test)]
mod test_support;

pub use container_exit::ContainerExitSink;
pub use dispatch::dispatch_commands;
pub use exit_watcher::{AgentExitReason, AgentExitSink, AgentPane, ExitWatcherManager};
pub use grpc_service::{WorkerDaemonServiceImpl, WorkerIdentity};
pub use init::{
    InitInstructionsManager, InitSettingsManager, InitSkillsManager, InstructionInputs,
    ProjectInstruction, ProjectInstructionMode, StartupHooksManager, WorkerInitInputs,
};
pub use init_git_hooks::InitGitHooksManager;
pub use init_skill_hooks::InitSkillHooksManager;
pub use paths::WorkerPaths;
pub use skill_layout::{SkillLayout, UR_PLUGIN_NAME};
