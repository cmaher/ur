use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub mod stream;

// -- Shared socket-path resolution --

/// Environment variable that overrides the config directory (default: `~/.ur`).
const UR_CONFIG_ENV: &str = "UR_CONFIG";

/// Name of the UDS socket file within the config directory.
const SOCKET_FILENAME: &str = "ur.sock";

/// Return the default socket path derived from `$UR_CONFIG` (or `~/.ur`).
///
/// Used by `urd`, `ur`, and `agent_tools` so they all agree on the same
/// path without an explicit CLI flag.
pub fn default_socket_path() -> PathBuf {
    let config_dir = if let Ok(val) = std::env::var(UR_CONFIG_ENV) {
        PathBuf::from(val)
    } else {
        dirs::home_dir()
            .expect("cannot determine home directory")
            .join(".ur")
    };
    config_dir.join(SOCKET_FILENAME)
}

// -- Streaming types --

/// Incremental output chunk from a command execution.
/// Sent over a side-channel Unix socket, not via tarpc.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum CommandOutput {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    Exit(i32),
}

// -- Request types --

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AskHumanRequest {
    pub process_id: String,
    pub question: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExecGitRequest {
    pub args: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ReportStatusRequest {
    pub process_id: String,
    pub status: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TicketReadRequest {
    pub ticket_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TicketSpawnRequest {
    pub parent_id: String,
    pub title: String,
    pub description: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TicketNoteRequest {
    pub ticket_id: String,
    pub note: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerBuildRequest {
    pub tag: String,
    pub dockerfile: String,
    pub context: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerRunRequest {
    pub image_id: String,
    pub name: String,
    pub cpus: u32,
    pub memory: String,
    pub volumes: Vec<(String, String)>,
    pub socket_mounts: Vec<(String, String)>,
    pub workdir: Option<String>,
    pub command: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerIdRequest {
    pub container_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerExecRequest {
    pub container_id: String,
    pub command: Vec<String>,
    pub workdir: Option<String>,
}

// -- Response types --

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AskHumanResponse {
    pub answer: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GitResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TicketReadResponse {
    pub content: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TicketSpawnResponse {
    pub ticket_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerBuildResponse {
    pub image_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerRunResponse {
    pub container_id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ContainerExecResponse {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Response from a streaming command execution RPC.
/// The client connects to `stream_socket` to receive `CommandOutput` frames.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StreamingExecResponse {
    /// Path to a Unix socket that will emit length-delimited bincode `CommandOutput` frames.
    pub stream_socket: String,
}

// -- Service trait --

#[tarpc::service]
pub trait UrAgentBridge {
    async fn ping() -> String;
    async fn ask_human(req: AskHumanRequest) -> Result<AskHumanResponse, String>;
    async fn exec_git(req: ExecGitRequest) -> Result<GitResponse, String>;
    /// Streaming variant of exec_git. Returns a socket path; the client connects
    /// to it to receive `CommandOutput` frames as the command runs.
    async fn exec_git_stream(req: ExecGitRequest) -> Result<StreamingExecResponse, String>;
    async fn report_status(req: ReportStatusRequest) -> Result<(), String>;
    async fn ticket_read(req: TicketReadRequest) -> Result<TicketReadResponse, String>;
    async fn ticket_spawn(req: TicketSpawnRequest) -> Result<TicketSpawnResponse, String>;
    async fn ticket_note(req: TicketNoteRequest) -> Result<(), String>;
    async fn container_build(req: ContainerBuildRequest) -> Result<ContainerBuildResponse, String>;
    async fn container_run(req: ContainerRunRequest) -> Result<ContainerRunResponse, String>;
    async fn container_stop(req: ContainerIdRequest) -> Result<(), String>;
    async fn container_rm(req: ContainerIdRequest) -> Result<(), String>;
    async fn container_exec(req: ContainerExecRequest) -> Result<ContainerExecResponse, String>;
}
