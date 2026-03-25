//! Server-internal client for the TicketService gRPC interface.
//!
//! Workflow handlers use this to create child tickets for CI failures, merge
//! conflicts, and merge rejections. Calls go through the `TicketService` trait
//! (not `TicketRepo` directly) to maintain the logical service boundary.

use tonic::Request;
use tracing::info;

use ur_rpc::proto::ticket::ticket_service_server::TicketService;
use ur_rpc::proto::ticket::{
    CreateTicketRequest, GetTicketRequest, ListTicketsRequest, SetMetaRequest,
};

use crate::grpc_ticket::TicketServiceHandler;

/// Issue types used as metadata key suffixes for workflow issue tickets.
pub mod issue_type {
    pub const CI_FAILURE: &str = "ci_failure";
    pub const MERGE_CONFLICT: &str = "merge_conflict";
    pub const MERGE_REJECTION: &str = "merge_rejection";
}

/// Metadata key prefix for workflow issue tickets.
/// Full key format: `workflow_issue:{issue_type}`.
const META_KEY_PREFIX: &str = "workflow_issue";

/// Builds the metadata key for a given issue type (e.g., `workflow_issue:ci_failure`).
fn meta_key(issue_type: &str) -> String {
    format!("{META_KEY_PREFIX}:{issue_type}")
}

/// Server-internal TicketService client for creating workflow issue tickets.
///
/// Wraps `TicketServiceHandler` and calls it in-process via the `TicketService`
/// trait, avoiding a network round-trip while maintaining the service boundary.
#[derive(Clone)]
pub struct TicketClient {
    handler: TicketServiceHandler,
}

impl TicketClient {
    pub fn new(handler: TicketServiceHandler) -> Self {
        Self { handler }
    }

    /// Create a child ticket representing a workflow issue (CI failure, merge
    /// conflict, or merge rejection).
    ///
    /// Deduplicates: if an open child of `parent_id` already has a matching
    /// `workflow_issue:{issue_type}` metadata key, the existing ticket ID is
    /// returned instead of creating a duplicate.
    pub async fn create_workflow_issue_ticket(
        &self,
        parent_id: &str,
        issue_type: &str,
        title: &str,
        body: &str,
    ) -> Result<String, anyhow::Error> {
        let key = meta_key(issue_type);

        // Dedup: look for an existing open child with this metadata key.
        if let Some(existing_id) = self.find_existing_issue(parent_id, &key).await? {
            info!(
                parent_id = %parent_id,
                issue_type = %issue_type,
                existing_id = %existing_id,
                "workflow issue ticket already exists, skipping creation"
            );
            return Ok(existing_id);
        }

        // Derive project from the parent ticket ID prefix (e.g., "ur-abc12" → "ur").
        let project = parent_id.split('-').next().unwrap_or("ur").to_owned();

        let create_resp = self
            .handler
            .create_ticket(Request::new(CreateTicketRequest {
                project,
                ticket_type: "task".to_owned(),
                status: String::new(),
                priority: 2,
                parent_id: Some(parent_id.to_owned()),
                title: title.to_owned(),
                body: body.to_owned(),
                id: None,
                created_at: None,
                wip: false,
            }))
            .await
            .map_err(|s| anyhow::anyhow!("create_ticket failed: {}", s.message()))?;

        let ticket_id = create_resp.into_inner().id;

        // Set the workflow_issue metadata key for dedup identification.
        self.handler
            .set_meta(Request::new(SetMetaRequest {
                ticket_id: ticket_id.clone(),
                key,
                value: "true".to_owned(),
            }))
            .await
            .map_err(|s| anyhow::anyhow!("set_meta failed: {}", s.message()))?;

        info!(
            parent_id = %parent_id,
            issue_type = %issue_type,
            ticket_id = %ticket_id,
            "created workflow issue ticket"
        );

        Ok(ticket_id)
    }

    /// Search for an existing open child ticket of `parent_id` that has the
    /// given metadata key set.
    async fn find_existing_issue(
        &self,
        parent_id: &str,
        meta_key: &str,
    ) -> Result<Option<String>, anyhow::Error> {
        // List tickets matching the metadata key+value. The metadata query
        // path returns minimal fields (no parent_id), so we fetch full
        // details for each candidate to check parentage.
        let candidates_resp = self
            .handler
            .list_tickets(Request::new(ListTicketsRequest {
                project: None,
                ticket_type: None,
                status: Some("open".to_owned()),
                meta_key: Some(meta_key.to_owned()),
                meta_value: Some("true".to_owned()),
                tree_root_id: None,
            }))
            .await
            .map_err(|s| anyhow::anyhow!("list_tickets failed: {}", s.message()))?;

        for candidate in candidates_resp.into_inner().tickets {
            let detail = self
                .handler
                .get_ticket(Request::new(GetTicketRequest {
                    id: candidate.id.clone(),
                    activity_author_filter: None,
                }))
                .await
                .map_err(|s| anyhow::anyhow!("get_ticket failed: {}", s.message()))?;

            if let Some(ticket) = detail.into_inner().ticket
                && ticket.parent_id == parent_id
                && ticket.status == "open"
            {
                return Ok(Some(ticket.id));
            }
        }

        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    use tempfile::TempDir;
    use ur_db::{DatabaseManager, GraphManager, TicketRepo, WorkflowRepo};

    async fn setup() -> (TicketClient, TicketRepo, TempDir) {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("test.db");
        let db = DatabaseManager::open(&db_path.to_string_lossy())
            .await
            .expect("open test db");
        let graph_manager = GraphManager::new(db.pool().clone());
        let repo = TicketRepo::new(db.pool().clone(), graph_manager);
        let workflow_repo = WorkflowRepo::new(db.pool().clone());

        let handler = TicketServiceHandler {
            ticket_repo: repo.clone(),
            workflow_repo,
            valid_projects: HashSet::new(),
            transition_tx: None,
            cancel_tx: None,
            ui_event_poller: None,
        };

        let client = TicketClient::new(handler);
        (client, repo, tmp)
    }

    #[tokio::test]
    async fn create_workflow_issue_ticket_creates_child_with_metadata() {
        let (client, repo, _tmp) = setup().await;

        // Create a parent ticket first.
        let parent = ur_db::NewTicket {
            id: Some("ur-parent".to_owned()),
            project: "ur".to_owned(),
            type_: "task".to_owned(),
            priority: 0,
            parent_id: None,
            title: "Parent ticket".to_owned(),
            body: String::new(),
            status: None,
            lifecycle_status: None,
            branch: None,
            created_at: None,
        };
        repo.create_ticket(&parent).await.unwrap();

        let ticket_id = client
            .create_workflow_issue_ticket(
                "ur-parent",
                issue_type::CI_FAILURE,
                "CI failed on main",
                "Build step exited with code 1",
            )
            .await
            .unwrap();

        assert!(!ticket_id.is_empty());

        // Verify the child ticket exists with correct parent.
        let child = repo.get_ticket(&ticket_id).await.unwrap().unwrap();
        assert_eq!(child.parent_id.as_deref(), Some("ur-parent"));
        assert_eq!(child.status, "open");

        // Verify metadata was set.
        let meta = repo.get_meta(&ticket_id, "ticket").await.unwrap();
        assert_eq!(
            meta.get("workflow_issue:ci_failure").map(String::as_str),
            Some("true")
        );
    }

    #[tokio::test]
    async fn create_workflow_issue_ticket_deduplicates() {
        let (client, repo, _tmp) = setup().await;

        // Create parent.
        let parent = ur_db::NewTicket {
            id: Some("ur-parent2".to_owned()),
            project: "ur".to_owned(),
            type_: "task".to_owned(),
            priority: 0,
            parent_id: None,
            title: "Parent".to_owned(),
            body: String::new(),
            status: None,
            lifecycle_status: None,
            branch: None,
            created_at: None,
        };
        repo.create_ticket(&parent).await.unwrap();

        // First creation.
        let id1 = client
            .create_workflow_issue_ticket(
                "ur-parent2",
                issue_type::MERGE_CONFLICT,
                "Merge conflict",
                "Conflict in src/main.rs",
            )
            .await
            .unwrap();

        // Second creation with same issue type — should return same ID.
        let id2 = client
            .create_workflow_issue_ticket(
                "ur-parent2",
                issue_type::MERGE_CONFLICT,
                "Merge conflict (new)",
                "Conflict in src/lib.rs",
            )
            .await
            .unwrap();

        assert_eq!(id1, id2);
    }

    #[tokio::test]
    async fn different_issue_types_create_separate_tickets() {
        let (client, repo, _tmp) = setup().await;

        // Create parent.
        let parent = ur_db::NewTicket {
            id: Some("ur-parent3".to_owned()),
            project: "ur".to_owned(),
            type_: "task".to_owned(),
            priority: 0,
            parent_id: None,
            title: "Parent".to_owned(),
            body: String::new(),
            status: None,
            lifecycle_status: None,
            branch: None,
            created_at: None,
        };
        repo.create_ticket(&parent).await.unwrap();

        let ci_id = client
            .create_workflow_issue_ticket("ur-parent3", issue_type::CI_FAILURE, "CI failed", "body")
            .await
            .unwrap();

        let merge_id = client
            .create_workflow_issue_ticket(
                "ur-parent3",
                issue_type::MERGE_REJECTION,
                "Merge rejected",
                "body",
            )
            .await
            .unwrap();

        assert_ne!(ci_id, merge_id);
    }
}
