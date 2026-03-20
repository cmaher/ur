pub mod database;
pub mod graph;
pub mod knowledge_repo;
pub mod model;
pub mod snapshot;
pub mod ticket_repo;
pub mod worker_repo;

#[cfg(test)]
mod tests;

pub use database::DatabaseManager;
pub use graph::GraphManager;
pub use knowledge_repo::KnowledgeRepo;
pub use model::{
    Activity, AgentStatus, DispatchableTicket, Edge, EdgeKind, Knowledge, KnowledgeFilter,
    KnowledgeSummary, KnowledgeUpdate, LifecycleStatus, MetadataMatchTicket, NewKnowledge,
    NewTicket, Slot, Ticket, TicketFilter, TicketStatus, TicketUpdate, Worker, WorkerSlot,
    Workflow, WorkflowEvent, WorkflowIntent,
};
pub use snapshot::SnapshotManager;
pub use ticket_repo::TicketRepo;
pub use worker_repo::{SlotReconcileResult, WorkerReconcileResult, WorkerRepo};
