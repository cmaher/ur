pub mod database;
pub mod snapshot;

#[cfg(test)]
mod tests;

pub use database::DatabaseManager;
pub use snapshot::SnapshotManager;

// Re-exports from workflow_db for backward compatibility.
pub use workflow_db::{
    AgentStatus, Slot, SlotReconcileResult, UiEventRepo, Worker, WorkerReconcileResult, WorkerRepo,
    WorkerSlot, Workflow, WorkflowEvent, WorkflowEventRow, WorkflowIntent, WorkflowRepo,
};
pub mod model {
    pub use workflow_db::model::*;
}
pub mod workflow_repo {
    pub use workflow_db::workflow_repo::*;
}
pub mod ui_event_repo {
    pub use workflow_db::ui_event_repo::*;
}
pub mod worker_repo {
    pub use workflow_db::worker_repo::*;
}
