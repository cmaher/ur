use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::context::TuiContext;
use crate::v2::cmd::{Cmd, FetchCmd};
use crate::v2::components::ticket_table::render_ticket_table;
use crate::v2::input::{FooterCommand, InputHandler, InputResult};
use crate::v2::model::{LoadState, Model, TicketListData};
use crate::v2::msg::{GotoTarget, Msg, NavMsg, OverlayMsg, TicketOpMsg};
use crate::v2::navigation::PageId;

/// Render the tickets list page into the given content area.
///
/// Delegates to the TicketTable component for the actual table rendering,
/// showing loading/error states as appropriate.
pub fn render_tickets_list(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    match &model.ticket_list.data {
        LoadState::NotLoaded | LoadState::Loading => {
            render_message(area, buf, ctx, "Loading...");
        }
        LoadState::Error(msg) => {
            render_message(area, buf, ctx, &format!("Error: {msg}"));
        }
        LoadState::Loaded(_) => {
            render_ticket_table(&model.ticket_list.table, area, buf, ctx);
        }
    }
}

/// Render a centered message for loading/error states.
fn render_message(area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
    let style = Style::default()
        .fg(ctx.theme.base_content)
        .bg(ctx.theme.base_100);
    Paragraph::new(Line::raw(msg))
        .style(style)
        .render(area, buf);
}

/// Handle ticket table navigation messages for the ticket list page.
///
/// Delegates navigation (up/down/page) to the TicketTableModel, issues fetch
/// commands when pagination changes, and pushes TicketDetail on selection.
pub fn handle_ticket_table_nav(mut model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    match nav_msg {
        NavMsg::TicketTableNavigate { delta } => {
            if delta > 0 {
                model.ticket_list.table.navigate_down();
            } else {
                model.ticket_list.table.navigate_up();
            }
            (model, vec![])
        }
        NavMsg::TicketTablePageRight => {
            if model.ticket_list.table.page_right() {
                let cmd = build_ticket_list_fetch_cmd(&model);
                (model, vec![cmd])
            } else {
                (model, vec![])
            }
        }
        NavMsg::TicketTablePageLeft => {
            if model.ticket_list.table.page_left() {
                let cmd = build_ticket_list_fetch_cmd(&model);
                (model, vec![cmd])
            } else {
                (model, vec![])
            }
        }
        NavMsg::TicketListRefresh => {
            model.ticket_list.data = LoadState::Loading;
            let cmd = build_ticket_list_fetch_cmd(&model);
            (model, vec![cmd])
        }
        NavMsg::TicketTableSelect => handle_select(model),
        NavMsg::TicketListPriority => handle_priority(model),
        NavMsg::TicketListClose => handle_close(model),
        NavMsg::TicketListOpen => handle_open(model),
        NavMsg::TicketListDispatch => handle_dispatch(model),
        NavMsg::TicketListDesign => handle_design(model),
        NavMsg::TicketListGoto => handle_goto(model),
        NavMsg::TicketListCreate => crate::v2::create_ticket::start_create_flow(model),
        _ => (model, vec![]),
    }
}

/// Apply loaded ticket data to the ticket list model.
///
/// Called from the data handler when TicketsLoaded arrives. Populates the
/// TicketTableModel with the fetched tickets and total count.
pub fn apply_tickets_data(model: &mut Model, data: TicketListData) {
    model.ticket_list.table.tickets = data.tickets;
    model.ticket_list.table.total_count = data.total_count;
    // Clamp selection if the new page has fewer rows
    let count = model.ticket_list.table.tickets.len();
    if count > 0 && model.ticket_list.table.selected_row >= count {
        model.ticket_list.table.selected_row = count - 1;
    }
}

/// Build a fetch command for the ticket list using the current table pagination
/// and filter state.
pub fn build_ticket_list_fetch_cmd(model: &Model) -> Cmd {
    let table = &model.ticket_list.table;
    let filters = &model.ticket_filters;
    Cmd::Fetch(FetchCmd::Tickets {
        page_size: Some(table.page_size as i32),
        offset: Some((table.current_page * table.page_size) as i32),
        include_children: Some(filters.show_children),
        statuses: filters.statuses.clone(),
    })
}

/// Push TicketDetail for the selected ticket.
fn handle_select(mut model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = model.ticket_list.table.selected_ticket() {
        let ticket_id = ticket.id.clone();
        let page = PageId::TicketDetail { ticket_id };
        let mut nav = std::mem::replace(
            &mut model.navigation_model,
            crate::v2::navigation::NavigationModel::initial(),
        );
        let cmds = nav.push(page, &mut model);
        model.navigation_model = nav;
        (model, cmds)
    } else {
        (model, vec![])
    }
}

/// Open the priority picker for the selected ticket.
fn handle_priority(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = model.ticket_list.table.selected_ticket() {
        let msg = Msg::Overlay(OverlayMsg::OpenPriorityPicker {
            ticket_id: ticket.id.clone(),
            current_priority: ticket.priority,
        });
        crate::v2::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Close the selected ticket, prompting for force close if it has open children.
fn handle_close(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = model.ticket_list.table.selected_ticket() {
        let open_children = ticket.children_total - ticket.children_completed;
        if open_children > 0 {
            let msg = Msg::Overlay(OverlayMsg::OpenForceCloseConfirm {
                ticket_id: ticket.id.clone(),
                open_children,
            });
            crate::v2::update::update(model, msg)
        } else {
            let msg = Msg::TicketOp(TicketOpMsg::Close {
                ticket_id: ticket.id.clone(),
            });
            crate::v2::update::update(model, msg)
        }
    } else {
        (model, vec![])
    }
}

/// Re-open the selected ticket (set status back to "open").
fn handle_open(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = model.ticket_list.table.selected_ticket() {
        let msg = Msg::TicketOp(TicketOpMsg::Open {
            ticket_id: ticket.id.clone(),
        });
        crate::v2::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Dispatch the selected ticket.
fn handle_dispatch(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = model.ticket_list.table.selected_ticket() {
        let msg = Msg::TicketOp(TicketOpMsg::Dispatch {
            ticket_id: ticket.id.clone(),
            project_key: ticket.project.clone(),
            image_id: String::new(),
        });
        crate::v2::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Launch a design worker for the selected ticket.
fn handle_design(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = model.ticket_list.table.selected_ticket() {
        let msg = Msg::TicketOp(TicketOpMsg::LaunchDesign {
            ticket_id: ticket.id.clone(),
            project_key: ticket.project.clone(),
            image_id: String::new(),
        });
        crate::v2::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Open the goto menu for the selected ticket.
fn handle_goto(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = model.ticket_list.table.selected_ticket() {
        let targets = build_ticket_goto_targets(&ticket.id);
        let msg = Msg::Overlay(OverlayMsg::OpenGotoMenu { targets });
        crate::v2::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Input handler for the tickets list page.
///
/// Handles ticket-specific actions: Create (C), Dispatch (D), Open/reopen (O),
/// Priority (P), Design (S), Close (X), Goto (g),
/// Refresh (r), Filter (*), Settings (,),
/// plus TicketTable navigation (j/k/h/l/Enter).
///
/// This is a root page handler: it is not pushed onto the input stack but
/// dispatched directly from `dispatch_root_page_key` when the current page
/// is TicketList.
pub struct TicketListHandler;

impl InputHandler for TicketListHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        // TicketTable navigation keys first (no modifiers)
        if key.modifiers == KeyModifiers::NONE
            && let Some(msg) = handle_table_key(key.code)
        {
            return InputResult::Capture(msg);
        }

        // Ticket operation keys (Shift + letter)
        if let Some(msg) = handle_operation_key(key) {
            return InputResult::Capture(msg);
        }

        // Lowercase action keys
        if key.modifiers == KeyModifiers::NONE
            && let Some(msg) = handle_action_key(key.code)
        {
            return InputResult::Capture(msg);
        }

        InputResult::Bubble
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            // Capitals alphabetical
            FooterCommand {
                key_label: "C".to_string(),
                description: "Create".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "D".to_string(),
                description: "Dispatch".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "O".to_string(),
                description: "Open".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "P".to_string(),
                description: "Priority".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "S".to_string(),
                description: "Design".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "X".to_string(),
                description: "Close".to_string(),
                common: false,
            },
            // Lowercase alphabetical
            FooterCommand {
                key_label: "g".to_string(),
                description: "Goto".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "r".to_string(),
                description: "Refresh".to_string(),
                common: false,
            },
            // Symbols
            FooterCommand {
                key_label: "Space".to_string(),
                description: "Details".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "*".to_string(),
                description: "Filter".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: ",".to_string(),
                description: "Settings".to_string(),
                common: false,
            },
            // Common (right side)
            FooterCommand {
                key_label: "j/k".to_string(),
                description: "Navigate".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: "h/l".to_string(),
                description: "Page".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: "Enter".to_string(),
                description: "Select".to_string(),
                common: true,
            },
        ]
    }

    fn name(&self) -> &str {
        "ticket_list"
    }
}

/// Handle TicketTable navigation keys (no modifiers).
fn handle_table_key(code: KeyCode) -> Option<Msg> {
    match code {
        KeyCode::Char('k') | KeyCode::Up => {
            Some(Msg::Nav(NavMsg::TicketTableNavigate { delta: -1 }))
        }
        KeyCode::Char('j') | KeyCode::Down => {
            Some(Msg::Nav(NavMsg::TicketTableNavigate { delta: 1 }))
        }
        KeyCode::Char('h') | KeyCode::Left => Some(Msg::Nav(NavMsg::TicketTablePageLeft)),
        KeyCode::Char('l') | KeyCode::Right => Some(Msg::Nav(NavMsg::TicketTablePageRight)),
        KeyCode::Char(' ') | KeyCode::Enter => Some(Msg::Nav(NavMsg::TicketTableSelect)),
        _ => None,
    }
}

/// Handle Shift+letter operation keys for ticket actions.
fn handle_operation_key(key: KeyEvent) -> Option<Msg> {
    if !key.modifiers.contains(KeyModifiers::SHIFT) {
        return None;
    }

    match key.code {
        KeyCode::Char('P') => Some(Msg::Nav(NavMsg::TicketListPriority)),
        KeyCode::Char('X') => Some(Msg::Nav(NavMsg::TicketListClose)),
        KeyCode::Char('O') => Some(Msg::Nav(NavMsg::TicketListOpen)),
        KeyCode::Char('D') => Some(Msg::Nav(NavMsg::TicketListDispatch)),
        KeyCode::Char('S') => Some(Msg::Nav(NavMsg::TicketListDesign)),
        KeyCode::Char('C') => Some(Msg::Nav(NavMsg::TicketListCreate)),
        _ => None,
    }
}

/// Handle lowercase action keys.
fn handle_action_key(code: KeyCode) -> Option<Msg> {
    match code {
        KeyCode::Char('r') => Some(Msg::Nav(NavMsg::TicketListRefresh)),
        KeyCode::Char('g') => Some(Msg::Nav(NavMsg::TicketListGoto)),
        KeyCode::Char('*') => Some(Msg::Overlay(OverlayMsg::OpenFilterMenu)),
        KeyCode::Char(',') => Some(Msg::Overlay(OverlayMsg::OpenSettings {
            custom_theme_names: vec![],
        })),
        _ => None,
    }
}

/// Build goto targets for the currently selected ticket.
pub fn build_ticket_goto_targets(ticket_id: &str) -> Vec<GotoTarget> {
    vec![
        GotoTarget {
            label: "Ticket Detail".to_string(),
            screen: "ticket".to_string(),
            id: ticket_id.to_string(),
        },
        GotoTarget {
            label: "Flow Details".to_string(),
            screen: "flow".to_string(),
            id: ticket_id.to_string(),
        },
        GotoTarget {
            label: "Worker".to_string(),
            screen: "worker".to_string(),
            id: ticket_id.to_string(),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use ur_rpc::proto::ticket::Ticket;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn plain_key(code: KeyCode) -> KeyEvent {
        make_key(code, KeyModifiers::NONE)
    }

    fn make_ticket(id: &str, title: &str) -> Ticket {
        Ticket {
            id: id.to_string(),
            ticket_type: "task".to_string(),
            status: "open".to_string(),
            priority: 2,
            parent_id: String::new(),
            title: title.to_string(),
            body: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            project: "test".to_string(),
            branch: String::new(),
            depth: 0,
            children_total: 0,
            children_completed: 0,
            dispatch_status: String::new(),
        }
    }

    // ── Handler key tests ────────────────────────────────────────────

    #[test]
    fn handler_captures_j_as_navigate_down() {
        let handler = TicketListHandler;
        match handler.handle_key(plain_key(KeyCode::Char('j'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableNavigate { delta: 1 })) => {}
            other => panic!("expected navigate down, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_k_as_navigate_up() {
        let handler = TicketListHandler;
        match handler.handle_key(plain_key(KeyCode::Char('k'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableNavigate { delta: -1 })) => {}
            other => panic!("expected navigate up, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_space_as_select() {
        let handler = TicketListHandler;
        match handler.handle_key(plain_key(KeyCode::Char(' '))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableSelect)) => {}
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_enter_as_select() {
        let handler = TicketListHandler;
        match handler.handle_key(plain_key(KeyCode::Enter)) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableSelect)) => {}
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_h_as_page_left() {
        let handler = TicketListHandler;
        match handler.handle_key(plain_key(KeyCode::Char('h'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTablePageLeft)) => {}
            other => panic!("expected page left, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_l_as_page_right() {
        let handler = TicketListHandler;
        match handler.handle_key(plain_key(KeyCode::Char('l'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTablePageRight)) => {}
            other => panic!("expected page right, got {other:?}"),
        }
    }

    #[test]
    fn handler_bubbles_f_key() {
        let handler = TicketListHandler;
        assert!(matches!(
            handler.handle_key(plain_key(KeyCode::Char('f'))),
            InputResult::Bubble
        ));
    }

    #[test]
    fn handler_bubbles_w_key() {
        let handler = TicketListHandler;
        assert!(matches!(
            handler.handle_key(plain_key(KeyCode::Char('w'))),
            InputResult::Bubble
        ));
    }

    #[test]
    fn handler_captures_shift_c_as_create() {
        let handler = TicketListHandler;
        let key = make_key(KeyCode::Char('C'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketListCreate)) => {}
            other => panic!("expected ticket list create, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_shift_p_as_priority() {
        let handler = TicketListHandler;
        let key = make_key(KeyCode::Char('P'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketListPriority)) => {}
            other => panic!("expected ticket list priority, got {other:?}"),
        }
    }

    #[test]
    fn handler_bubbles_unrecognized() {
        let handler = TicketListHandler;
        assert!(matches!(
            handler.handle_key(plain_key(KeyCode::Char('z'))),
            InputResult::Bubble
        ));
    }

    #[test]
    fn handler_has_footer_commands() {
        let handler = TicketListHandler;
        let cmds = handler.footer_commands();
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| c.description == "Create"));
        assert!(cmds.iter().any(|c| c.description == "Dispatch"));
        assert!(cmds.iter().any(|c| c.description == "Close"));
        assert!(
            cmds.iter()
                .any(|c| c.description == "Filter" && c.key_label == "*")
        );
        assert!(cmds.iter().any(|c| c.description == "Priority"));
        assert!(
            cmds.iter()
                .any(|c| c.description == "Details" && c.key_label == "Space")
        );
    }

    #[test]
    fn handler_name() {
        let handler = TicketListHandler;
        assert_eq!(handler.name(), "ticket_list");
    }

    // ── apply_tickets_data tests ─────────────────────────────────────

    #[test]
    fn apply_tickets_data_populates_table() {
        let mut model = Model::initial();
        let tickets = vec![
            make_ticket("ur-001", "First"),
            make_ticket("ur-002", "Second"),
        ];
        let data = TicketListData {
            tickets: tickets.clone(),
            total_count: 2,
        };
        apply_tickets_data(&mut model, data);
        assert_eq!(model.ticket_list.table.tickets.len(), 2);
        assert_eq!(model.ticket_list.table.total_count, 2);
    }

    #[test]
    fn apply_tickets_data_clamps_selection() {
        let mut model = Model::initial();
        model.ticket_list.table.selected_row = 5;
        let data = TicketListData {
            tickets: vec![make_ticket("ur-001", "Only one")],
            total_count: 1,
        };
        apply_tickets_data(&mut model, data);
        assert_eq!(model.ticket_list.table.selected_row, 0);
    }

    // ── Navigation handler tests ─────────────────────────────────────

    #[test]
    fn navigate_down_increments_selection() {
        let mut model = Model::initial();
        model.ticket_list.table.tickets =
            vec![make_ticket("ur-001", "A"), make_ticket("ur-002", "B")];
        model.ticket_list.table.total_count = 2;
        let (new_model, cmds) =
            handle_ticket_table_nav(model, NavMsg::TicketTableNavigate { delta: 1 });
        assert_eq!(new_model.ticket_list.table.selected_row, 1);
        assert!(cmds.is_empty());
    }

    #[test]
    fn navigate_up_decrements_selection() {
        let mut model = Model::initial();
        model.ticket_list.table.tickets =
            vec![make_ticket("ur-001", "A"), make_ticket("ur-002", "B")];
        model.ticket_list.table.total_count = 2;
        model.ticket_list.table.selected_row = 1;
        let (new_model, cmds) =
            handle_ticket_table_nav(model, NavMsg::TicketTableNavigate { delta: -1 });
        assert_eq!(new_model.ticket_list.table.selected_row, 0);
        assert!(cmds.is_empty());
    }

    #[test]
    fn select_pushes_ticket_detail() {
        let mut model = Model::initial();
        model.ticket_list.table.tickets = vec![make_ticket("ur-abc", "Test")];
        model.ticket_list.table.total_count = 1;
        let (new_model, cmds) = handle_ticket_table_nav(model, NavMsg::TicketTableSelect);
        assert_eq!(
            new_model.navigation_model.current_page(),
            &PageId::TicketDetail {
                ticket_id: "ur-abc".to_string()
            }
        );
        assert!(!cmds.is_empty());
    }

    #[test]
    fn select_on_empty_is_noop() {
        let model = Model::initial();
        let (_, cmds) = handle_ticket_table_nav(model, NavMsg::TicketTableSelect);
        assert!(cmds.is_empty());
    }

    // ── Build fetch command tests ────────────────────────────────────

    #[test]
    fn build_fetch_uses_pagination() {
        let mut model = Model::initial();
        model.ticket_list.table.page_size = 10;
        model.ticket_list.table.current_page = 2;
        let cmd = build_ticket_list_fetch_cmd(&model);
        match cmd {
            Cmd::Fetch(FetchCmd::Tickets {
                page_size,
                offset,
                include_children,
                statuses,
            }) => {
                assert_eq!(page_size, Some(10));
                assert_eq!(offset, Some(20));
                assert_eq!(include_children, Some(false));
                assert!(!statuses.is_empty());
            }
            other => panic!("expected Fetch(Tickets), got {other:?}"),
        }
    }

    // ── Goto targets tests ───────────────────────────────────────────

    #[test]
    fn goto_targets_include_standard_options() {
        let targets = build_ticket_goto_targets("ur-abc");
        assert_eq!(targets.len(), 3);
        assert!(targets.iter().any(|t| t.label == "Ticket Detail"));
        assert!(targets.iter().any(|t| t.label == "Flow Details"));
        assert!(targets.iter().any(|t| t.label == "Worker"));
    }
}
