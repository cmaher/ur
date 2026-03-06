use serde::{Deserialize, Serialize};

// -- Request types --

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AskHumanRequest {
    pub process_id: String,
    pub question: String,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ExecGitRequest {
    pub process_id: String,
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

// -- Service trait --

#[tarpc::service]
pub trait UrAgentBridge {
    async fn ask_human(req: AskHumanRequest) -> Result<AskHumanResponse, String>;
    async fn exec_git(req: ExecGitRequest) -> Result<GitResponse, String>;
    async fn report_status(req: ReportStatusRequest) -> Result<(), String>;
    async fn ticket_read(req: TicketReadRequest) -> Result<TicketReadResponse, String>;
    async fn ticket_spawn(req: TicketSpawnRequest) -> Result<TicketSpawnResponse, String>;
    async fn ticket_note(req: TicketNoteRequest) -> Result<(), String>;
    async fn container_build(req: ContainerBuildRequest) -> Result<ContainerBuildResponse, String>;
    async fn container_run(req: ContainerRunRequest) -> Result<ContainerRunResponse, String>;
    async fn container_stop(req: ContainerIdRequest) -> Result<(), String>;
    async fn container_rm(req: ContainerIdRequest) -> Result<(), String>;
}
