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

// -- Service trait --

#[tarpc::service]
pub trait UrAgentBridge {
    async fn ask_human(req: AskHumanRequest) -> Result<AskHumanResponse, String>;
    async fn exec_git(req: ExecGitRequest) -> Result<GitResponse, String>;
    async fn report_status(req: ReportStatusRequest) -> Result<(), String>;
    async fn ticket_read(req: TicketReadRequest) -> Result<TicketReadResponse, String>;
    async fn ticket_spawn(req: TicketSpawnRequest) -> Result<TicketSpawnResponse, String>;
    async fn ticket_note(req: TicketNoteRequest) -> Result<(), String>;
}
