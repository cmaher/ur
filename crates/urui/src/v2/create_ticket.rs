//! Create ticket flow orchestration for v2.
//!
//! The create ticket flow shells out to $EDITOR with a frontmatter template,
//! parses the result, then shows the CreateActionMenu overlay for the user
//! to choose the action (Create, Dispatch, Design, Abandon).
//!
//! Entry points:
//! - `start_create_flow`: begins the flow from the ticket list (no parent)
//! - `start_create_child_flow`: begins the flow from ticket detail (with parent ID)

use super::cmd::Cmd;
use super::model::Model;

/// Start the create ticket flow (top-level ticket, no parent).
///
/// Emits `Cmd::SpawnEditor` which causes the TEA loop to break out, run
/// the editor, and re-enter with the parsed result.
pub fn start_create_flow(model: Model) -> (Model, Vec<Cmd>) {
    (
        model,
        vec![Cmd::SpawnEditor {
            parent_id: None,
            project: None,
            content: None,
        }],
    )
}

/// Start the create child ticket flow (child of the ticket on the detail page).
///
/// Emits `Cmd::SpawnEditor` with the parent's ID and project pre-filled.
pub fn start_create_child_flow(model: Model) -> (Model, Vec<Cmd>) {
    let (parent_id, project) = match &model.ticket_detail {
        Some(detail) => {
            let parent_id = detail.ticket_id.clone();
            let project = detail
                .data
                .data()
                .and_then(|d| d.detail.ticket.as_ref().map(|t| t.project.clone()))
                .unwrap_or_default();
            (parent_id, project)
        }
        None => return (model, vec![]),
    };

    let project_opt = if project.is_empty() {
        None
    } else {
        Some(project)
    };

    (
        model,
        vec![Cmd::SpawnEditor {
            parent_id: Some(parent_id),
            project: project_opt,
            content: None,
        }],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::model::{
        LoadState, Model, TicketDetailData, TicketDetailModel, TicketTableModel,
    };

    #[test]
    fn start_create_flow_emits_spawn_editor() {
        let model = Model::initial();
        let (_, cmds) = start_create_flow(model);
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Cmd::SpawnEditor {
                parent_id, project, ..
            } => {
                assert!(parent_id.is_none());
                assert!(project.is_none());
            }
            other => panic!("expected SpawnEditor, got {other:?}"),
        }
    }

    fn make_detail_model(ticket_id: &str, project: &str) -> TicketDetailModel {
        use ur_rpc::proto::ticket::{GetTicketResponse, Ticket};
        TicketDetailModel {
            ticket_id: ticket_id.to_string(),
            data: LoadState::Loaded(TicketDetailData {
                detail: GetTicketResponse {
                    ticket: Some(Ticket {
                        id: ticket_id.to_string(),
                        title: "Parent".to_string(),
                        body: String::new(),
                        created_at: String::new(),
                        updated_at: String::new(),
                        project: project.to_string(),
                        status: "open".to_string(),
                        priority: 0,
                        parent_id: String::new(),
                        ticket_type: "task".to_string(),
                        children_completed: 0,
                        children_total: 0,
                        depth: 0,
                        branch: String::new(),
                        dispatch_status: String::new(),
                    }),
                    activities: vec![],
                    metadata: vec![],
                },
                children: vec![],
                total_children: 0,
            }),
            activities: LoadState::NotLoaded,
            children_table: TicketTableModel::empty(),
            show_closed: false,
        }
    }

    #[test]
    fn start_create_child_flow_with_project() {
        let mut model = Model::initial();
        model.ticket_detail = Some(make_detail_model("ur-abc", "ur"));
        let (_, cmds) = start_create_child_flow(model);
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            Cmd::SpawnEditor {
                parent_id, project, ..
            } => {
                assert_eq!(parent_id.as_deref(), Some("ur-abc"));
                assert_eq!(project.as_deref(), Some("ur"));
            }
            other => panic!("expected SpawnEditor, got {other:?}"),
        }
    }

    #[test]
    fn start_create_child_flow_without_detail() {
        let model = Model::initial();
        let (_, cmds) = start_create_child_flow(model);
        assert!(cmds.is_empty());
    }
}
