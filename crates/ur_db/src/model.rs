// Shared data types for ur_db.

pub struct Ticket {
    pub id: String,
    pub type_: String,
    pub status: String,
    pub priority: i32,
    pub parent_id: Option<String>,
    pub title: String,
    pub body: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Default)]
pub struct NewTicket {
    pub id: String,
    pub type_: String,
    pub priority: i32,
    pub parent_id: Option<String>,
    pub title: String,
    pub body: String,
    /// If set, use this status instead of the default "open".
    pub status: Option<String>,
    /// If set, use this timestamp instead of now.
    pub created_at: Option<String>,
}

pub struct TicketUpdate {
    pub status: Option<String>,
    pub priority: Option<i32>,
    pub title: Option<String>,
    pub body: Option<String>,
    pub parent_id: Option<Option<String>>, // Some(None) to clear
}

pub struct TicketFilter {
    pub status: Option<String>,
    pub type_: Option<String>,
    pub parent_id: Option<String>,
}

pub struct Activity {
    pub id: String,
    pub ticket_id: String,
    pub timestamp: String,
    pub author: String,
    pub message: String,
}

pub struct DispatchableTicket {
    pub id: String,
    pub title: String,
    pub priority: i32,
    pub type_: String,
}

pub struct MetadataMatchTicket {
    pub id: String,
    pub title: String,
    pub type_: String,
    pub status: String,
    pub key: String,
    pub value: String,
}

pub struct Edge {
    pub source_id: String,
    pub target_id: String,
    pub kind: EdgeKind,
}

pub enum EdgeKind {
    Blocks,
    RelatesTo,
}
