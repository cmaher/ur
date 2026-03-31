//! Create ticket flow orchestration for v2.
//!
//! The create ticket flow is a multi-step overlay sequence:
//! 1. ProjectInput — user enters the project key
//! 2. TitleInput — user enters the ticket title
//! 3. CreateActionMenu — user chooses the action (Create, Dispatch, Design, Abandon)
//!
//! Each step produces an `OverlayMsg` that is handled here to advance the flow.
//! The `CreateTicketState` in the model accumulates data across steps.
//!
//! Entry points:
//! - `start_create_flow`: begins the flow from the ticket list (no parent)
//! - `start_create_child_flow`: begins the flow from ticket detail (with parent ID)

use super::cmd::Cmd;
use super::model::{CreateTicketState, Model};
use super::msg::{Msg, OverlayMsg, PendingTicket};

/// Start the create ticket flow (top-level ticket, no parent).
///
/// Opens the ProjectInput overlay and initializes the create ticket state.
pub fn start_create_flow(model: Model) -> (Model, Vec<Cmd>) {
    let mut model = model;
    model.create_ticket_state = Some(CreateTicketState {
        project: String::new(),
        parent_id: None,
    });
    super::update::update(model, Msg::Overlay(OverlayMsg::OpenProjectInput))
}

/// Start the create child ticket flow (child of the ticket on the detail page).
///
/// Opens the ProjectInput overlay and initializes the create ticket state
/// with the parent ticket's ID and project pre-filled.
pub fn start_create_child_flow(model: Model) -> (Model, Vec<Cmd>) {
    let (parent_id, project) = match &model.ticket_detail {
        Some(detail) => {
            let parent_id = detail.ticket_id.clone();
            let project = detail
                .data
                .data()
                .map(|d| d.detail.ticket.as_ref().map(|t| t.project.clone()))
                .flatten()
                .unwrap_or_default();
            (parent_id, project)
        }
        None => return (model, vec![]),
    };

    let mut model = model;
    model.create_ticket_state = Some(CreateTicketState {
        project: project.clone(),
        parent_id: Some(parent_id),
    });

    // If we already have a project from the parent, skip to title input.
    if !project.is_empty() {
        return super::update::update(model, Msg::Overlay(OverlayMsg::OpenTitleInput));
    }

    super::update::update(model, Msg::Overlay(OverlayMsg::OpenProjectInput))
}

/// Handle the project input being submitted during the create ticket flow.
///
/// Stores the project in the create ticket state and advances to the title
/// input step.
pub fn handle_project_submitted(mut model: Model, project: String) -> (Model, Vec<Cmd>) {
    if let Some(ref mut state) = model.create_ticket_state {
        state.project = project;
    } else {
        // No active create flow — discard.
        return (model, vec![]);
    }

    super::update::update(model, Msg::Overlay(OverlayMsg::OpenTitleInput))
}

/// Handle the title input being submitted during the create ticket flow.
///
/// Builds the `PendingTicket` from the accumulated state and opens the
/// `CreateActionMenu` overlay for the user to choose the final action.
pub fn handle_title_submitted(model: Model, title: String) -> (Model, Vec<Cmd>) {
    let pending = match &model.create_ticket_state {
        Some(state) => PendingTicket {
            project: state.project.clone(),
            title,
            priority: 0,
            parent_id: state.parent_id.clone(),
        },
        None => return (model, vec![]),
    };

    // Clear the create state — the pending ticket now carries all the data.
    let mut model = model;
    model.create_ticket_state = None;

    super::update::update(
        model,
        Msg::Overlay(OverlayMsg::OpenCreateActionMenu { pending }),
    )
}

/// Cancel the create ticket flow, clearing any accumulated state.
pub fn cancel_create_flow(mut model: Model) -> (Model, Vec<Cmd>) {
    model.create_ticket_state = None;
    (model, vec![])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v2::model::{
        ActiveOverlay, LoadState, Model, TicketDetailData, TicketDetailModel, TicketTableModel,
    };
    use crate::v2::msg::OverlayMsg;

    #[test]
    fn start_create_flow_sets_state_and_opens_project_input() {
        let model = Model::initial();
        let (new_model, _) = start_create_flow(model);
        assert!(new_model.create_ticket_state.is_some());
        let state = new_model.create_ticket_state.as_ref().unwrap();
        assert!(state.project.is_empty());
        assert!(state.parent_id.is_none());
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::ProjectInput { .. })
        ));
    }

    #[test]
    fn handle_project_submitted_stores_project_and_opens_title_input() {
        let model = Model::initial();
        let (model, _) = start_create_flow(model);
        let (new_model, _) = handle_project_submitted(model, "ur".to_string());
        // Project should be stored (or consumed since we move to title input)
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::TitleInput { .. })
        ));
    }

    #[test]
    fn handle_title_submitted_opens_create_action_menu() {
        let mut model = Model::initial();
        model.create_ticket_state = Some(CreateTicketState {
            project: "ur".to_string(),
            parent_id: None,
        });
        let (new_model, _) = handle_title_submitted(model, "Fix the bug".to_string());
        assert!(new_model.create_ticket_state.is_none());
        match &new_model.active_overlay {
            Some(ActiveOverlay::CreateActionMenu { pending, .. }) => {
                assert_eq!(pending.project, "ur");
                assert_eq!(pending.title, "Fix the bug");
                assert_eq!(pending.priority, 0);
                assert!(pending.parent_id.is_none());
            }
            other => panic!("expected CreateActionMenu, got {other:?}"),
        }
    }

    #[test]
    fn handle_title_submitted_with_parent_id() {
        let mut model = Model::initial();
        model.create_ticket_state = Some(CreateTicketState {
            project: "ur".to_string(),
            parent_id: Some("ur-abc".to_string()),
        });
        let (new_model, _) = handle_title_submitted(model, "Child task".to_string());
        match &new_model.active_overlay {
            Some(ActiveOverlay::CreateActionMenu { pending, .. }) => {
                assert_eq!(pending.parent_id, Some("ur-abc".to_string()));
            }
            other => panic!("expected CreateActionMenu, got {other:?}"),
        }
    }

    #[test]
    fn cancel_create_flow_clears_state() {
        let mut model = Model::initial();
        model.create_ticket_state = Some(CreateTicketState {
            project: "ur".to_string(),
            parent_id: None,
        });
        let (new_model, _) = cancel_create_flow(model);
        assert!(new_model.create_ticket_state.is_none());
    }

    #[test]
    fn handle_project_submitted_no_active_flow() {
        let model = Model::initial();
        let (new_model, cmds) = handle_project_submitted(model, "ur".to_string());
        assert!(new_model.create_ticket_state.is_none());
        assert!(cmds.is_empty());
    }

    #[test]
    fn handle_title_submitted_no_active_flow() {
        let model = Model::initial();
        let (new_model, cmds) = handle_title_submitted(model, "title".to_string());
        assert!(new_model.create_ticket_state.is_none());
        assert!(cmds.is_empty());
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
        let (new_model, _) = start_create_child_flow(model);
        // Should skip to title input since project is known
        assert!(matches!(
            new_model.active_overlay,
            Some(ActiveOverlay::TitleInput { .. })
        ));
        let state = new_model.create_ticket_state.as_ref().unwrap();
        assert_eq!(state.project, "ur");
        assert_eq!(state.parent_id, Some("ur-abc".to_string()));
    }

    #[test]
    fn start_create_child_flow_without_detail() {
        let model = Model::initial();
        let (new_model, cmds) = start_create_child_flow(model);
        assert!(new_model.create_ticket_state.is_none());
        assert!(cmds.is_empty());
    }
}
