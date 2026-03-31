use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};
use ur_rpc::proto::ticket::Ticket;

use crate::context::TuiContext;
use crate::widgets::{MiniProgressBar, ThemedTable};

use super::super::input::{FooterCommand, InputHandler, InputResult};
use super::super::model::TicketTableModel;
use super::super::msg::{Msg, NavMsg};

/// Column headers for the ticket table — matches v1 layout exactly.
const HEADERS: [&str; 6] = ["ID", "Status", "P", "Progress", "", "Title"];

/// Column index of the progress count label.
const PROGRESS_COUNT_COL: usize = 3;
/// Column index of the progress bar.
const PROGRESS_BAR_COL: usize = 4;

/// Build the column width constraints for the ticket table.
/// Matches v1 exactly: ID(12), Status(8), P(8), Progress(8), Bar(10), Title(fill).
fn table_widths() -> Vec<Constraint> {
    vec![
        Constraint::Length(12),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(8),
        Constraint::Length(10),
        Constraint::Fill(1),
    ]
}

/// Derive the display label for the Status column from the ticket's state.
///
/// Same logic as v1: dispatched tickets show "Dispatched", closed show
/// "Closed", everything else shows "Open".
fn dispatch_label(ticket: &Ticket) -> String {
    if !ticket.dispatch_status.is_empty() {
        "Dispatched".to_string()
    } else if ticket.status == "closed" {
        "Closed".to_string()
    } else {
        "Open".to_string()
    }
}

/// Compute progress values for a ticket.
///
/// Leaf tickets (children_total=0): 0/1 if open, 1/1 if closed.
/// Parent tickets: children_completed/children_total from proto fields.
pub fn ticket_progress(ticket: &Ticket) -> (u32, u32) {
    if ticket.children_total > 0 {
        (
            ticket.children_completed as u32,
            ticket.children_total as u32,
        )
    } else if ticket.status == "closed" {
        (1, 1)
    } else {
        (0, 1)
    }
}

/// Build table row strings from tickets. Progress columns are left empty
/// because progress bars are rendered directly to the buffer with themed
/// colors in [`render_progress_bars`].
fn build_rows(tickets: &[Ticket]) -> Vec<Vec<String>> {
    tickets
        .iter()
        .map(|t| {
            vec![
                t.id.clone(),
                dispatch_label(t),
                t.priority.to_string(),
                String::new(), // progress count placeholder
                String::new(), // progress bar placeholder
                t.title.clone(),
            ]
        })
        .collect()
}

/// Render the ticket table from the given model slice into the area.
///
/// Returns the scroll offset applied by the underlying ThemedTable so that
/// progress bars can be rendered at the correct Y positions.
pub fn render_ticket_table(
    table_model: &TicketTableModel,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
) -> usize {
    if table_model.tickets.is_empty() {
        render_empty_message(area, buf, ctx, "No tickets");
        return 0;
    }

    let widths = table_widths();
    let rows = build_rows(&table_model.tickets);

    let themed_table = ThemedTable {
        headers: HEADERS.to_vec(),
        rows,
        selected: Some(table_model.selected_row),
        widths: widths.clone(),
        page_info: Some(table_model.page_info()),
    };

    let scroll_offset = themed_table.render(area, buf, ctx);

    render_progress_bars(table_model, area, buf, ctx, &widths, scroll_offset);

    scroll_offset
}

/// Render a centered message (for empty/loading/error states).
fn render_empty_message(area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
    let style = Style::default()
        .fg(ctx.theme.base_content)
        .bg(ctx.theme.base_100);
    Paragraph::new(Line::raw(msg))
        .style(style)
        .render(area, buf);
}

/// Render mini progress bars and count labels over the placeholder columns.
///
/// Matches the v1 implementation: resolves column positions from the same
/// constraints, then renders bar and count separately for each visible row.
fn render_progress_bars(
    table_model: &TicketTableModel,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    widths: &[Constraint],
    scroll_offset: usize,
) {
    // The table block has a 1-cell border on each side.
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1, // skip top border
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let col_areas = Layout::horizontal(widths.to_vec()).split(inner);
    let bar_area = col_areas.get(PROGRESS_BAR_COL);
    let count_area = col_areas.get(PROGRESS_COUNT_COL);

    if bar_area.is_none() && count_area.is_none() {
        return;
    }

    // First row inside inner is the header; data rows start at inner.y + 1.
    let data_start_y = inner.y + 1;

    for (i, ticket) in table_model.tickets.iter().enumerate().skip(scroll_offset) {
        let row_y = data_start_y + (i - scroll_offset) as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let (completed, total) = ticket_progress(ticket);
        let is_selected = i == table_model.selected_row;
        let row_bg = if is_selected {
            ctx.theme.primary
        } else if i % 2 == 0 {
            ctx.theme.base_100
        } else {
            ctx.theme.base_200
        };
        let row_fg = if is_selected {
            ctx.theme.primary_content
        } else {
            ctx.theme.base_content
        };

        let bar = MiniProgressBar { completed, total };

        if let Some(ba) = bar_area {
            let cell = Rect {
                x: ba.x,
                y: row_y,
                width: ba.width,
                height: 1,
            };
            bar.render_bar(cell, buf, &ctx.theme, row_bg);
        }

        if let Some(ca) = count_area {
            let cell = Rect {
                x: ca.x,
                y: row_y,
                width: ca.width,
                height: 1,
            };
            bar.render_label_styled(cell, buf, row_fg, row_bg);
        }
    }
}

/// Input handler for the ticket table.
///
/// Captures hjkl/arrow keys for navigation, left/right for pagination,
/// and Enter for selection. This handler is reusable: the same instance
/// works on both the ticket list page and the ticket detail children table.
pub struct TicketTableHandler;

impl InputHandler for TicketTableHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        if key.modifiers != KeyModifiers::NONE {
            return InputResult::Bubble;
        }

        match key.code {
            // Navigation: up
            KeyCode::Char('k') | KeyCode::Up => {
                InputResult::Capture(Msg::Nav(NavMsg::TicketTableNavigate { delta: -1 }))
            }
            // Navigation: down
            KeyCode::Char('j') | KeyCode::Down => {
                InputResult::Capture(Msg::Nav(NavMsg::TicketTableNavigate { delta: 1 }))
            }
            // Pagination: previous page
            KeyCode::Char('h') | KeyCode::Left => {
                InputResult::Capture(Msg::Nav(NavMsg::TicketTablePageLeft))
            }
            // Pagination: next page
            KeyCode::Char('l') | KeyCode::Right => {
                InputResult::Capture(Msg::Nav(NavMsg::TicketTablePageRight))
            }
            // Selection
            KeyCode::Enter => InputResult::Capture(Msg::Nav(NavMsg::TicketTableSelect)),
            _ => InputResult::Bubble,
        }
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "j/k".to_string(),
                description: "Navigate".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "h/l".to_string(),
                description: "Page".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "Enter".to_string(),
                description: "Select".to_string(),
                common: false,
            },
        ]
    }

    fn name(&self) -> &str {
        "ticket_table"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

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

    fn make_model_with_tickets(n: usize) -> TicketTableModel {
        let tickets: Vec<Ticket> = (0..n)
            .map(|i| make_ticket(&format!("ur-{i:04}"), &format!("Ticket {i}")))
            .collect();
        TicketTableModel {
            total_count: n as i32,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
            tickets,
        }
    }

    // ── Navigation tests ──────────────────────────────────────────────

    #[test]
    fn navigate_down_increments_selection() {
        let mut m = make_model_with_tickets(5);
        m.navigate_down();
        assert_eq!(m.selected_row, 1);
        m.navigate_down();
        assert_eq!(m.selected_row, 2);
    }

    #[test]
    fn navigate_down_clamps_at_last_row() {
        let mut m = make_model_with_tickets(3);
        m.selected_row = 2;
        m.navigate_down();
        assert_eq!(m.selected_row, 2);
    }

    #[test]
    fn navigate_up_decrements_selection() {
        let mut m = make_model_with_tickets(5);
        m.selected_row = 3;
        m.navigate_up();
        assert_eq!(m.selected_row, 2);
    }

    #[test]
    fn navigate_up_clamps_at_zero() {
        let mut m = make_model_with_tickets(5);
        m.selected_row = 0;
        m.navigate_up();
        assert_eq!(m.selected_row, 0);
    }

    #[test]
    fn navigate_on_empty_is_safe() {
        let mut m = make_model_with_tickets(0);
        m.navigate_down();
        assert_eq!(m.selected_row, 0);
        m.navigate_up();
        assert_eq!(m.selected_row, 0);
    }

    // ── Pagination tests ──────────────────────────────────────────────

    #[test]
    fn page_right_advances_and_resets_selection() {
        let mut m = make_model_with_tickets(5);
        m.page_size = 2;
        m.total_count = 5;
        m.selected_row = 1;
        assert!(m.page_right());
        assert_eq!(m.current_page, 1);
        assert_eq!(m.selected_row, 0);
    }

    #[test]
    fn page_right_clamps_at_last_page() {
        let mut m = make_model_with_tickets(5);
        m.page_size = 5;
        m.total_count = 5;
        m.current_page = 0;
        assert!(!m.page_right());
        assert_eq!(m.current_page, 0);
    }

    #[test]
    fn page_left_decrements_and_resets_selection() {
        let mut m = make_model_with_tickets(5);
        m.page_size = 2;
        m.total_count = 5;
        m.current_page = 2;
        m.selected_row = 1;
        assert!(m.page_left());
        assert_eq!(m.current_page, 1);
        assert_eq!(m.selected_row, 0);
    }

    #[test]
    fn page_left_clamps_at_zero() {
        let mut m = make_model_with_tickets(5);
        m.current_page = 0;
        assert!(!m.page_left());
        assert_eq!(m.current_page, 0);
    }

    #[test]
    fn total_pages_calculation() {
        let mut m = make_model_with_tickets(0);
        m.page_size = 10;
        m.total_count = 0;
        assert_eq!(m.total_pages(), 1);

        m.total_count = 10;
        assert_eq!(m.total_pages(), 1);

        m.total_count = 11;
        assert_eq!(m.total_pages(), 2);

        m.total_count = 20;
        assert_eq!(m.total_pages(), 2);

        m.total_count = 21;
        assert_eq!(m.total_pages(), 3);
    }

    // ── Selection tests ───────────────────────────────────────────────

    #[test]
    fn selected_ticket_returns_correct_ticket() {
        let m = make_model_with_tickets(3);
        let t = m.selected_ticket().unwrap();
        assert_eq!(t.id, "ur-0000");
    }

    #[test]
    fn selected_ticket_after_navigate() {
        let mut m = make_model_with_tickets(3);
        m.navigate_down();
        m.navigate_down();
        let t = m.selected_ticket().unwrap();
        assert_eq!(t.id, "ur-0002");
    }

    #[test]
    fn selected_ticket_empty_returns_none() {
        let m = make_model_with_tickets(0);
        assert!(m.selected_ticket().is_none());
    }

    // ── Page size tests ───────────────────────────────────────────────

    #[test]
    fn update_page_size_from_area_height() {
        let mut m = make_model_with_tickets(5);
        m.page_size = 20;
        // area_height 13 -> 13 - 3 chrome = 10 rows
        assert!(m.update_page_size(13));
        assert_eq!(m.page_size, 10);
        assert_eq!(m.current_page, 0);
        assert_eq!(m.selected_row, 0);
    }

    #[test]
    fn update_page_size_no_change_returns_false() {
        let mut m = make_model_with_tickets(5);
        m.page_size = 10;
        assert!(!m.update_page_size(13)); // 13 - 3 = 10, same
    }

    #[test]
    fn update_page_size_zero_area_returns_false() {
        let mut m = make_model_with_tickets(5);
        assert!(!m.update_page_size(0));
        assert!(!m.update_page_size(3)); // 3 - 3 = 0
    }

    // ── Page info string ──────────────────────────────────────────────

    #[test]
    fn page_info_format() {
        let mut m = make_model_with_tickets(0);
        m.page_size = 10;
        m.total_count = 25;
        m.current_page = 1;
        assert_eq!(m.page_info(), "Page 2/3 (25 total)");
    }

    // ── Handler tests ─────────────────────────────────────────────────

    #[test]
    fn handler_captures_j_as_navigate_down() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Char('j'));
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableNavigate { delta: 1 })) => {}
            other => panic!("expected navigate down, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_k_as_navigate_up() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Char('k'));
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableNavigate { delta: -1 })) => {}
            other => panic!("expected navigate up, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_arrow_down() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Down);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableNavigate { delta: 1 })) => {}
            other => panic!("expected navigate down, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_arrow_up() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Up);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableNavigate { delta: -1 })) => {}
            other => panic!("expected navigate up, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_h_as_page_left() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Char('h'));
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTablePageLeft)) => {}
            other => panic!("expected page left, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_l_as_page_right() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Char('l'));
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTablePageRight)) => {}
            other => panic!("expected page right, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_left_arrow_as_page_left() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Left);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTablePageLeft)) => {}
            other => panic!("expected page left, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_right_arrow_as_page_right() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Right);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTablePageRight)) => {}
            other => panic!("expected page right, got {other:?}"),
        }
    }

    #[test]
    fn handler_captures_enter_as_select() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Enter);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TicketTableSelect)) => {}
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn handler_bubbles_unrecognized_keys() {
        let handler = TicketTableHandler;
        let key = plain_key(KeyCode::Char('x'));
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn handler_bubbles_modified_keys() {
        let handler = TicketTableHandler;
        let key = make_key(KeyCode::Char('j'), KeyModifiers::CONTROL);
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn handler_has_footer_commands() {
        let handler = TicketTableHandler;
        let commands = handler.footer_commands();
        assert!(!commands.is_empty());
        assert!(commands.iter().any(|c| c.description == "Navigate"));
        assert!(commands.iter().any(|c| c.description == "Page"));
        assert!(commands.iter().any(|c| c.description == "Select"));
    }

    #[test]
    fn handler_name() {
        let handler = TicketTableHandler;
        assert_eq!(handler.name(), "ticket_table");
    }

    // ── dispatch_label tests ──────────────────────────────────────────

    #[test]
    fn dispatch_label_open() {
        let t = make_ticket("ur-001", "test");
        assert_eq!(dispatch_label(&t), "Open");
    }

    #[test]
    fn dispatch_label_closed() {
        let mut t = make_ticket("ur-001", "test");
        t.status = "closed".to_string();
        assert_eq!(dispatch_label(&t), "Closed");
    }

    #[test]
    fn dispatch_label_dispatched() {
        let mut t = make_ticket("ur-001", "test");
        t.dispatch_status = "implementing".to_string();
        assert_eq!(dispatch_label(&t), "Dispatched");
    }

    // ── ticket_progress tests ─────────────────────────────────────────

    #[test]
    fn progress_leaf_open() {
        let t = make_ticket("ur-001", "test");
        assert_eq!(ticket_progress(&t), (0, 1));
    }

    #[test]
    fn progress_leaf_closed() {
        let mut t = make_ticket("ur-001", "test");
        t.status = "closed".to_string();
        assert_eq!(ticket_progress(&t), (1, 1));
    }

    #[test]
    fn progress_parent() {
        let mut t = make_ticket("ur-001", "test");
        t.children_total = 5;
        t.children_completed = 3;
        assert_eq!(ticket_progress(&t), (3, 5));
    }

    // ── build_rows tests ──────────────────────────────────────────────

    #[test]
    fn build_rows_produces_correct_columns() {
        let tickets = vec![make_ticket("ur-001", "First ticket")];
        let rows = build_rows(&tickets);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].len(), 6);
        assert_eq!(rows[0][0], "ur-001");
        assert_eq!(rows[0][1], "Open");
        assert_eq!(rows[0][2], "2");
        assert!(rows[0][3].is_empty()); // progress count placeholder
        assert!(rows[0][4].is_empty()); // progress bar placeholder
        assert_eq!(rows[0][5], "First ticket");
    }
}
