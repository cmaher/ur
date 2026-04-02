use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};
use ur_markdown::{MarkdownColors, render_markdown};

use crate::context::TuiContext;
use crate::v2::cmd::{Cmd, FetchCmd};
use crate::v2::components::ticket_table::render_ticket_table;
use crate::v2::input::{FooterCommand, InputHandler, InputResult};
use crate::v2::model::{LoadState, Model, TicketDetailData};
use crate::v2::msg::{GotoTarget, Msg, NavMsg, OverlayMsg, TicketOpMsg};
use crate::v2::navigation::PageId;
use crate::widgets::MiniProgressBar;

/// Render the ticket detail page into the given content area.
///
/// Layout: header (1 line) + body preview (5 lines) + children table (fill).
pub fn render_ticket_detail(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let Some(ref detail_model) = model.ticket_detail else {
        render_message(area, buf, ctx, "No ticket selected");
        return;
    };

    match &detail_model.data {
        LoadState::NotLoaded | LoadState::Loading => {
            render_message(area, buf, ctx, "Loading...");
        }
        LoadState::Error(msg) => {
            render_message(area, buf, ctx, &format!("Error: {msg}"));
        }
        LoadState::Loaded(data) => {
            render_loaded_detail(area, buf, ctx, model, data);
        }
    }
}

/// Render a loaded ticket detail with header, body preview, and children table.
fn render_loaded_detail(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    model: &Model,
    data: &TicketDetailData,
) {
    let chunks = Layout::vertical([
        Constraint::Length(1), // header line
        Constraint::Length(5), // body preview (4 lines + 1 for "...")
        Constraint::Min(3),    // children table
    ])
    .split(area);

    if let Some(ticket) = &data.detail.ticket {
        render_ticket_header(ticket, chunks[0], buf, ctx);
        render_body_preview(&ticket.body, chunks[1], buf, ctx);
    } else {
        let ticket_id = &model
            .ticket_detail
            .as_ref()
            .map_or("unknown", |d| &d.ticket_id);
        render_message(chunks[0], buf, ctx, ticket_id);
    }

    // Render children table using the TicketTable component
    if let Some(ref detail_model) = model.ticket_detail {
        if detail_model.children_table.tickets.is_empty()
            && !detail_model.data.is_loading()
            && detail_model.data.is_loaded()
        {
            render_message(chunks[2], buf, ctx, "No children");
        } else {
            render_ticket_table(&detail_model.children_table, chunks[2], buf, ctx);
        }
    }
}

/// Render the ticket header: ID, title (truncated), status, and progress bar.
fn render_ticket_header(
    ticket: &ur_rpc::proto::ticket::Ticket,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
) {
    let id_style = Style::default().fg(ctx.theme.accent);
    let title_style = Style::default().fg(ctx.theme.base_content);
    let status_style = Style::default().fg(ctx.theme.neutral_content);

    let status_label = if !ticket.dispatch_status.is_empty() {
        "Dispatched"
    } else if ticket.status == "closed" {
        "Closed"
    } else {
        "Open"
    };

    // Reserve space for progress bar on the right (10 chars)
    let progress_width = 10u16;
    let header_text_width = area.width.saturating_sub(progress_width + 1);

    let id_part = format!("{} ", ticket.id);
    let status_part = format!(" [{}]", status_label);
    let title_budget = (header_text_width as usize)
        .saturating_sub(id_part.len() + status_part.len())
        .max(1);
    let title_truncated = truncate_title(&ticket.title, title_budget);

    let spans = vec![
        Span::styled(id_part, id_style),
        Span::styled(title_truncated, title_style),
        Span::styled(status_part, status_style),
    ];
    let line = Line::from(spans);

    let text_area = Rect {
        x: area.x,
        y: area.y,
        width: header_text_width,
        height: 1,
    };
    Paragraph::new(line).render(text_area, buf);

    // Render the progress bar on the right
    let (completed, total) = crate::v2::components::ticket_table::ticket_progress(ticket);
    let bar = MiniProgressBar { completed, total };
    let bar_area = Rect {
        x: area.x + header_text_width + 1,
        y: area.y,
        width: progress_width,
        height: 1,
    };
    bar.render_bar(bar_area, buf, &ctx.theme, ctx.theme.base_100);
}

/// Truncate a title to fit within the given budget.
fn truncate_title(title: &str, budget: usize) -> String {
    if title.chars().count() > budget {
        let s: String = title.chars().take(budget.saturating_sub(1)).collect();
        format!("{s}\u{2026}")
    } else {
        title.to_string()
    }
}

/// Render the body preview: up to 4 rendered markdown lines, with "..." if truncated.
fn render_body_preview(body: &str, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    const MAX_LINES: usize = 4;

    let colors = markdown_colors(ctx);
    let all_lines = render_markdown(body, area.width as usize, &colors);
    let truncated = all_lines.len() > MAX_LINES;
    let preview_lines: Vec<Line<'static>> = all_lines.into_iter().take(MAX_LINES).collect();

    let style = Style::default()
        .fg(ctx.theme.base_content)
        .bg(ctx.theme.base_100);
    Paragraph::new(preview_lines).style(style).render(area, buf);

    if truncated && area.height > MAX_LINES as u16 {
        let dots_y = area.y + MAX_LINES as u16;
        let dots_style = Style::default()
            .fg(ctx.theme.neutral_content)
            .bg(ctx.theme.base_100);
        Paragraph::new(Line::raw("...")).style(dots_style).render(
            Rect {
                x: area.x,
                y: dots_y,
                width: area.width,
                height: 1,
            },
            buf,
        );
    }
}

/// Build `MarkdownColors` from the TUI theme.
fn markdown_colors(ctx: &TuiContext) -> MarkdownColors {
    MarkdownColors {
        text: ctx.theme.base_content,
        heading: ctx.theme.accent,
        code: ctx.theme.warning,
        dim: ctx.theme.secondary,
    }
}

/// Render a centered message for loading/error/empty states.
fn render_message(area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
    let style = Style::default()
        .fg(ctx.theme.base_content)
        .bg(ctx.theme.base_100);
    Paragraph::new(Line::raw(msg))
        .style(style)
        .render(area, buf);
}

/// Handle ticket detail navigation messages.
///
/// Delegates navigation (up/down/page) to the children TicketTableModel,
/// issues fetch commands when pagination changes, and pushes nested
/// TicketDetail on selection.
pub fn handle_ticket_detail_nav(mut model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    match nav_msg {
        NavMsg::TicketDetailNavigate { delta } => {
            if let Some(ref mut detail) = model.ticket_detail {
                if delta > 0 {
                    detail.children_table.navigate_down();
                } else {
                    detail.children_table.navigate_up();
                }
            }
            (model, vec![])
        }
        NavMsg::TicketDetailPageRight => {
            if let Some(ref mut detail) = model.ticket_detail
                && detail.children_table.page_right()
            {
                let cmd = build_detail_fetch_cmd(&model);
                return (model, vec![cmd]);
            }
            (model, vec![])
        }
        NavMsg::TicketDetailPageLeft => {
            if let Some(ref mut detail) = model.ticket_detail
                && detail.children_table.page_left()
            {
                let cmd = build_detail_fetch_cmd(&model);
                return (model, vec![cmd]);
            }
            (model, vec![])
        }
        NavMsg::TicketDetailRefresh => {
            if let Some(ref mut detail) = model.ticket_detail {
                detail.data = LoadState::Loading;
            }
            let cmd = build_detail_fetch_cmd(&model);
            (model, vec![cmd])
        }
        NavMsg::TicketDetailSelect => handle_detail_select(model),
        NavMsg::TicketDetailPriority => handle_detail_priority(model),
        NavMsg::TicketDetailClose => handle_detail_close(model),
        NavMsg::TicketDetailOpen => handle_detail_open(model),
        NavMsg::TicketDetailDispatch => handle_detail_dispatch(model),
        NavMsg::TicketDetailDispatchAll => handle_detail_dispatch_all(model),
        NavMsg::TicketDetailDesign => handle_detail_design(model),
        NavMsg::TicketDetailRedrive => handle_detail_redrive(model),
        NavMsg::TicketDetailGoto => handle_detail_goto(model),
        NavMsg::TicketDetailToggleClosed => handle_toggle_closed(model),
        NavMsg::TicketDetailOpenDescription => handle_open_description(model),
        NavMsg::TicketDetailOpenActivities => handle_open_activities(model),
        NavMsg::TicketDetailCreateChild => handle_create_child(model),
        NavMsg::TicketDetailEdit => handle_edit_parent(model),
        _ => (model, vec![]),
    }
}

/// Build a fetch command for the ticket detail using the current children
/// table pagination and filter state.
pub fn build_detail_fetch_cmd(model: &Model) -> Cmd {
    let Some(ref detail) = model.ticket_detail else {
        return Cmd::None;
    };
    let table = &detail.children_table;
    let child_status_filter = if detail.show_closed {
        None
    } else {
        Some("open,in_progress".to_string())
    };
    Cmd::batch(vec![
        Cmd::Fetch(FetchCmd::TicketDetail {
            ticket_id: detail.ticket_id.clone(),
            child_page_size: Some(table.page_size as i32),
            child_offset: Some((table.current_page * table.page_size) as i32),
            child_status_filter,
        }),
        Cmd::Fetch(FetchCmd::Activities {
            ticket_id: detail.ticket_id.clone(),
            author_filter: None,
        }),
    ])
}

/// Push a nested TicketDetail for the selected child.
fn handle_detail_select(mut model: Model) -> (Model, Vec<Cmd>) {
    let child_id = model
        .ticket_detail
        .as_ref()
        .and_then(|d| d.children_table.selected_ticket())
        .map(|t| t.id.clone());

    if let Some(ticket_id) = child_id {
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

/// Open the priority picker for the selected child.
fn handle_detail_priority(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = selected_child(&model) {
        let msg = Msg::Overlay(OverlayMsg::OpenPriorityPicker {
            ticket_id: ticket.id.clone(),
            current_priority: ticket.priority,
        });
        crate::v2::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Close the selected child, prompting for force close if it has open children.
fn handle_detail_close(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = selected_child(&model) {
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

/// Reopen the selected child.
fn handle_detail_open(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = selected_child(&model) {
        let ticket_id = ticket.id.clone();
        let msg = Msg::StatusShow(format!("Reopening {ticket_id}..."));
        let (model, _) = crate::v2::update::update(model, msg);
        (model, vec![Cmd::TicketOp(TicketOpMsg::Close { ticket_id })])
    } else {
        (model, vec![])
    }
}

/// Dispatch the selected child.
fn handle_detail_dispatch(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = selected_child(&model) {
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

/// Dispatch the parent ticket itself (dispatch all).
fn handle_detail_dispatch_all(model: Model) -> (Model, Vec<Cmd>) {
    let Some(ref detail) = model.ticket_detail else {
        return (model, vec![]);
    };
    let ticket_id = detail.ticket_id.clone();
    let project_key = detail
        .data
        .data()
        .and_then(|d| d.detail.ticket.as_ref())
        .map_or(String::new(), |t| t.project.clone());
    let msg = Msg::TicketOp(TicketOpMsg::DispatchAll {
        ticket_id,
        project_key,
        image_id: String::new(),
    });
    crate::v2::update::update(model, msg)
}

/// Launch a design worker for the selected child.
fn handle_detail_design(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = selected_child(&model) {
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

/// Redrive the selected child's workflow.
fn handle_detail_redrive(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = selected_child(&model) {
        let msg = Msg::TicketOp(TicketOpMsg::Redrive {
            ticket_id: ticket.id.clone(),
        });
        crate::v2::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Open the goto menu for the selected child.
fn handle_detail_goto(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ticket) = selected_child(&model) {
        let targets = build_child_goto_targets(&ticket.id);
        let msg = Msg::Overlay(OverlayMsg::OpenGotoMenu { targets });
        crate::v2::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Toggle show/hide closed children filter.
fn handle_toggle_closed(mut model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ref mut detail) = model.ticket_detail {
        detail.show_closed = !detail.show_closed;
        detail.children_table.current_page = 0;
        detail.children_table.selected_row = 0;
        detail.data = LoadState::Loading;
    }
    let cmd = build_detail_fetch_cmd(&model);
    (model, vec![cmd])
}

/// Open the description (body) of the parent ticket.
fn handle_open_description(mut model: Model) -> (Model, Vec<Cmd>) {
    let page = model.ticket_detail.as_ref().and_then(|d| {
        d.data.data().and_then(|data| {
            data.detail.ticket.as_ref().map(|t| PageId::TicketBody {
                ticket_id: t.id.clone(),
                title: t.title.clone(),
                body: t.body.clone(),
            })
        })
    });

    if let Some(page) = page {
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

/// Open the activities page for the parent ticket.
fn handle_open_activities(mut model: Model) -> (Model, Vec<Cmd>) {
    let page = model.ticket_detail.as_ref().and_then(|d| {
        d.data.data().and_then(|data| {
            data.detail
                .ticket
                .as_ref()
                .map(|t| PageId::TicketActivities {
                    ticket_id: t.id.clone(),
                    ticket_title: t.title.clone(),
                })
        })
    });

    if let Some(page) = page {
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

/// Edit the parent ticket in $EDITOR.
fn handle_edit_parent(model: Model) -> (Model, Vec<Cmd>) {
    let Some(ref detail) = model.ticket_detail else {
        return (model, vec![]);
    };
    let ticket_id = detail.ticket_id.clone();
    (model, vec![Cmd::EditTicket { ticket_id }])
}

/// Open the project input overlay for creating a child ticket.
fn handle_create_child(model: Model) -> (Model, Vec<Cmd>) {
    crate::v2::create_ticket::start_create_child_flow(model)
}

/// Get the currently selected child ticket, if any.
fn selected_child(model: &Model) -> Option<ur_rpc::proto::ticket::Ticket> {
    model
        .ticket_detail
        .as_ref()
        .and_then(|d| d.children_table.selected_ticket().cloned())
}

/// Build goto targets for a child ticket.
fn build_child_goto_targets(ticket_id: &str) -> Vec<GotoTarget> {
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

/// Input handler for the ticket detail page.
///
/// Handles ticket-specific actions on children: Dispatch All (A), Create child (C),
/// Dispatch (D), Open/reopen (O), Priority (P), Design (S), Redrive (V),
/// Close (X), activities (a), toggle-closed (c), description (d), goto (g),
/// refresh (r), plus children table navigation (j/k/h/l/Enter).
///
/// This is a root page handler dispatched directly from `dispatch_root_page_key`.
pub struct TicketDetailHandler;

impl InputHandler for TicketDetailHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        // Children table navigation keys (no modifiers)
        if key.modifiers == KeyModifiers::NONE
            && let Some(msg) = handle_detail_table_key(key.code)
        {
            return InputResult::Capture(msg);
        }

        // Shift+letter operation keys
        if let Some(msg) = handle_detail_operation_key(key) {
            return InputResult::Capture(msg);
        }

        // Lowercase action keys
        if key.modifiers == KeyModifiers::NONE
            && let Some(msg) = handle_detail_action_key(key.code)
        {
            return InputResult::Capture(msg);
        }

        InputResult::Bubble
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "A".to_string(),
                description: "Dispatch all".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "C".to_string(),
                description: "Create child".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "D".to_string(),
                description: "Dispatch".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "E".to_string(),
                description: "Edit".to_string(),
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
                key_label: "V".to_string(),
                description: "Redrive".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "X".to_string(),
                description: "Close".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "a".to_string(),
                description: "Activities".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "d".to_string(),
                description: "Description".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "c".to_string(),
                description: "Toggle closed".to_string(),
                common: false,
            },
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
        "ticket_detail"
    }
}

/// Handle children table navigation keys (no modifiers).
fn handle_detail_table_key(code: KeyCode) -> Option<Msg> {
    match code {
        KeyCode::Char('k') | KeyCode::Up => {
            Some(Msg::Nav(NavMsg::TicketDetailNavigate { delta: -1 }))
        }
        KeyCode::Char('j') | KeyCode::Down => {
            Some(Msg::Nav(NavMsg::TicketDetailNavigate { delta: 1 }))
        }
        KeyCode::Char('h') | KeyCode::Left => Some(Msg::Nav(NavMsg::TicketDetailPageLeft)),
        KeyCode::Char('l') | KeyCode::Right => Some(Msg::Nav(NavMsg::TicketDetailPageRight)),
        KeyCode::Enter => Some(Msg::Nav(NavMsg::TicketDetailSelect)),
        _ => None,
    }
}

/// Handle Shift+letter operation keys for ticket detail actions.
fn handle_detail_operation_key(key: KeyEvent) -> Option<Msg> {
    if !key.modifiers.contains(KeyModifiers::SHIFT) {
        return None;
    }

    match key.code {
        KeyCode::Char('A') => Some(Msg::Nav(NavMsg::TicketDetailDispatchAll)),
        KeyCode::Char('C') => Some(Msg::Nav(NavMsg::TicketDetailCreateChild)),
        KeyCode::Char('D') => Some(Msg::Nav(NavMsg::TicketDetailDispatch)),
        KeyCode::Char('E') => Some(Msg::Nav(NavMsg::TicketDetailEdit)),
        KeyCode::Char('O') => Some(Msg::Nav(NavMsg::TicketDetailOpen)),
        KeyCode::Char('P') => Some(Msg::Nav(NavMsg::TicketDetailPriority)),
        KeyCode::Char('S') => Some(Msg::Nav(NavMsg::TicketDetailDesign)),
        KeyCode::Char('V') => Some(Msg::Nav(NavMsg::TicketDetailRedrive)),
        KeyCode::Char('X') => Some(Msg::Nav(NavMsg::TicketDetailClose)),
        _ => None,
    }
}

/// Handle lowercase action keys for ticket detail.
fn handle_detail_action_key(code: KeyCode) -> Option<Msg> {
    match code {
        KeyCode::Char('a') => Some(Msg::Nav(NavMsg::TicketDetailOpenActivities)),
        KeyCode::Char('d') => Some(Msg::Nav(NavMsg::TicketDetailOpenDescription)),
        KeyCode::Char('c') => Some(Msg::Nav(NavMsg::TicketDetailToggleClosed)),
        KeyCode::Char('g') => Some(Msg::Nav(NavMsg::TicketDetailGoto)),
        KeyCode::Char('r') => Some(Msg::Nav(NavMsg::TicketDetailRefresh)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use ur_rpc::proto::ticket::{GetTicketResponse, Ticket};

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

    fn model_with_detail() -> Model {
        use crate::v2::model::{TicketDetailData, TicketDetailModel, TicketTableModel};

        let children = vec![
            make_ticket("ur-child-1", "Child 1"),
            make_ticket("ur-child-2", "Child 2"),
        ];
        let mut table = TicketTableModel::empty();
        table.tickets = children.clone();
        table.total_count = 2;

        let mut model = Model::initial();
        model.ticket_detail = Some(TicketDetailModel {
            ticket_id: "ur-parent".to_string(),
            data: LoadState::Loaded(TicketDetailData {
                detail: GetTicketResponse {
                    ticket: Some(make_ticket("ur-parent", "Parent ticket")),
                    ..Default::default()
                },
                children: children.clone(),
                total_children: 2,
            }),
            activities: LoadState::NotLoaded,
            children_table: table,
            show_closed: false,
        });
        model
    }

    // ── Handler key tests ────────────────────────────────────────────

    #[test]
    fn handler_captures_j_as_navigate_down() {
        let handler = TicketDetailHandler;
        match handler.handle_key(plain_key(KeyCode::Char('j'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketDetailNavigate { delta: 1 })) => {}
            other => panic!("expected navigate down, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_k_as_navigate_up() {
        let handler = TicketDetailHandler;
        match handler.handle_key(plain_key(KeyCode::Char('k'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketDetailNavigate { delta: -1 })) => {}
            other => panic!("expected navigate up, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_enter_as_select() {
        let handler = TicketDetailHandler;
        match handler.handle_key(plain_key(KeyCode::Enter)) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketDetailSelect)) => {}
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_c_as_toggle_closed() {
        let handler = TicketDetailHandler;
        match handler.handle_key(plain_key(KeyCode::Char('c'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketDetailToggleClosed)) => {}
            other => panic!("expected toggle closed, got {other:?}"),
        }
    }

    #[test]
    fn handler_bubbles_f_key() {
        let handler = TicketDetailHandler;
        assert!(matches!(
            handler.handle_key(plain_key(KeyCode::Char('f'))),
            InputResult::Bubble
        ));
    }

    #[test]
    fn handler_captures_a_as_activities() {
        let handler = TicketDetailHandler;
        match handler.handle_key(plain_key(KeyCode::Char('a'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketDetailOpenActivities)) => {}
            other => panic!("expected open activities, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_d_as_description() {
        let handler = TicketDetailHandler;
        match handler.handle_key(plain_key(KeyCode::Char('d'))) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketDetailOpenDescription)) => {}
            other => panic!("expected open description, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_shift_p_as_priority() {
        let handler = TicketDetailHandler;
        let key = make_key(KeyCode::Char('P'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketDetailPriority)) => {}
            other => panic!("expected priority, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_shift_a_as_dispatch_all() {
        let handler = TicketDetailHandler;
        let key = make_key(KeyCode::Char('A'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketDetailDispatchAll)) => {}
            other => panic!("expected dispatch all, got {other:?}"),
        }
    }

    #[test]
    fn handler_bubbles_unrecognized() {
        let handler = TicketDetailHandler;
        assert!(matches!(
            handler.handle_key(plain_key(KeyCode::Char('z'))),
            InputResult::Bubble
        ));
    }

    #[test]
    fn handler_has_footer_commands() {
        let handler = TicketDetailHandler;
        let cmds = handler.footer_commands();
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| c.description == "Dispatch all"));
        assert!(cmds.iter().any(|c| c.description == "Create child"));
        assert!(cmds.iter().any(|c| c.description == "Priority"));
        assert!(cmds.iter().any(|c| c.description == "Activities"));
        assert!(cmds.iter().any(|c| c.description == "Description"));
        assert!(cmds.iter().any(|c| c.description == "Toggle closed"));
    }

    #[test]
    fn handler_name() {
        let handler = TicketDetailHandler;
        assert_eq!(handler.name(), "ticket_detail");
    }

    // ── Navigation handler tests ─────────────────────────────────────

    #[test]
    fn navigate_down_increments_selection() {
        let model = model_with_detail();
        let (new_model, cmds) =
            handle_ticket_detail_nav(model, NavMsg::TicketDetailNavigate { delta: 1 });
        assert_eq!(
            new_model.ticket_detail.unwrap().children_table.selected_row,
            1
        );
        assert!(cmds.is_empty());
    }

    #[test]
    fn navigate_up_at_zero_stays() {
        let model = model_with_detail();
        let (new_model, cmds) =
            handle_ticket_detail_nav(model, NavMsg::TicketDetailNavigate { delta: -1 });
        assert_eq!(
            new_model.ticket_detail.unwrap().children_table.selected_row,
            0
        );
        assert!(cmds.is_empty());
    }

    #[test]
    fn select_pushes_nested_detail() {
        let model = model_with_detail();
        let (new_model, cmds) = handle_ticket_detail_nav(model, NavMsg::TicketDetailSelect);
        assert_eq!(
            new_model.navigation_model.current_page(),
            &PageId::TicketDetail {
                ticket_id: "ur-child-1".to_string()
            }
        );
        assert!(!cmds.is_empty());
    }

    #[test]
    fn toggle_closed_flips_flag() {
        let model = model_with_detail();
        assert!(!model.ticket_detail.as_ref().unwrap().show_closed);
        let (new_model, cmds) = handle_ticket_detail_nav(model, NavMsg::TicketDetailToggleClosed);
        assert!(new_model.ticket_detail.as_ref().unwrap().show_closed);
        assert!(!cmds.is_empty()); // should issue a re-fetch
    }

    #[test]
    fn toggle_closed_resets_pagination() {
        let mut model = model_with_detail();
        model
            .ticket_detail
            .as_mut()
            .unwrap()
            .children_table
            .current_page = 2;
        model
            .ticket_detail
            .as_mut()
            .unwrap()
            .children_table
            .selected_row = 3;
        let (new_model, _) = handle_ticket_detail_nav(model, NavMsg::TicketDetailToggleClosed);
        let detail = new_model.ticket_detail.unwrap();
        assert_eq!(detail.children_table.current_page, 0);
        assert_eq!(detail.children_table.selected_row, 0);
    }

    #[test]
    fn build_detail_fetch_cmd_includes_status_filter() {
        let model = model_with_detail();
        let cmd = build_detail_fetch_cmd(&model);
        match cmd {
            Cmd::Batch(cmds) => {
                let has_detail_fetch = cmds.iter().any(|c| {
                    matches!(
                        c,
                        Cmd::Fetch(FetchCmd::TicketDetail {
                            child_status_filter: Some(_),
                            ..
                        })
                    )
                });
                assert!(has_detail_fetch, "should include status filter");
            }
            _ => panic!("expected Batch, got {cmd:?}"),
        }
    }

    #[test]
    fn build_detail_fetch_cmd_no_filter_when_show_closed() {
        let mut model = model_with_detail();
        model.ticket_detail.as_mut().unwrap().show_closed = true;
        let cmd = build_detail_fetch_cmd(&model);
        match cmd {
            Cmd::Batch(cmds) => {
                let has_no_filter = cmds.iter().any(|c| {
                    matches!(
                        c,
                        Cmd::Fetch(FetchCmd::TicketDetail {
                            child_status_filter: None,
                            ..
                        })
                    )
                });
                assert!(
                    has_no_filter,
                    "should have no status filter when show_closed"
                );
            }
            _ => panic!("expected Batch, got {cmd:?}"),
        }
    }

    // ── Truncate title tests ─────────────────────────────────────────

    #[test]
    fn truncate_title_short_unchanged() {
        assert_eq!(truncate_title("Hello", 10), "Hello");
    }

    #[test]
    fn truncate_title_long_gets_ellipsis() {
        let result = truncate_title("Very long title here", 10);
        assert!(result.ends_with('\u{2026}'));
        assert!(result.chars().count() <= 10);
    }

    // ── Goto targets tests ───────────────────────────────────────────

    #[test]
    fn child_goto_targets_include_standard_options() {
        let targets = build_child_goto_targets("ur-abc");
        assert_eq!(targets.len(), 3);
        assert!(targets.iter().any(|t| t.label == "Ticket Detail"));
        assert!(targets.iter().any(|t| t.label == "Flow Details"));
        assert!(targets.iter().any(|t| t.label == "Worker"));
    }
}
