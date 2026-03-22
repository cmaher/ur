use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use ur_rpc::proto::ticket::Ticket;

use crate::context::TuiContext;
use crate::data::DataPayload;
use crate::keymap::Action;
use crate::page::{FooterCommand, Page, PageResult, TabId};
use crate::widgets::ThemedTable;

/// Internal state for the ticket data lifecycle.
#[derive(Debug, Clone)]
enum DataState {
    /// No data has been fetched yet.
    Loading,
    /// Data was fetched successfully.
    Loaded(Vec<Ticket>),
    /// Data fetch failed with the given error message.
    Error(String),
}

/// The Tickets tab page.
///
/// Displays all tickets in a themed table with columns: ID, Type, Status,
/// Priority, Title. Supports row selection (NavigateUp/Down) and client-side
/// pagination (PageLeft/Right).
pub struct TicketsPage {
    data_state: DataState,
    selected_row: usize,
    current_page: usize,
    page_size: usize,
}

impl TicketsPage {
    pub fn new() -> Self {
        Self {
            data_state: DataState::Loading,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
        }
    }

    /// Total number of pages given the current ticket count and page size.
    fn total_pages(&self) -> usize {
        let count = self.ticket_count();
        if count == 0 || self.page_size == 0 {
            return 1;
        }
        (count + self.page_size - 1) / self.page_size
    }

    /// Number of tickets in the current dataset.
    fn ticket_count(&self) -> usize {
        match &self.data_state {
            DataState::Loaded(tickets) => tickets.len(),
            _ => 0,
        }
    }

    /// Returns the slice of tickets visible on the current page.
    fn visible_tickets(&self) -> &[Ticket] {
        match &self.data_state {
            DataState::Loaded(tickets) => {
                let start = self.current_page * self.page_size;
                let end = (start + self.page_size).min(tickets.len());
                if start >= tickets.len() {
                    &[]
                } else {
                    &tickets[start..end]
                }
            }
            _ => &[],
        }
    }

    /// Build table row data from visible tickets.
    fn build_rows(&self) -> Vec<Vec<String>> {
        self.visible_tickets()
            .iter()
            .map(|t| {
                vec![
                    t.id.clone(),
                    t.ticket_type.clone(),
                    t.status.clone(),
                    t.priority.to_string(),
                    t.title.clone(),
                ]
            })
            .collect()
    }

    /// Render a centered message (used for loading/error states).
    fn render_message(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
        let style = Style::default()
            .fg(ctx.theme.base_content)
            .bg(ctx.theme.base_100);
        let paragraph = Paragraph::new(Line::raw(msg)).style(style);
        paragraph.render(area, buf);
    }

    /// Handle NavigateUp: move selection up within the current page.
    fn navigate_up(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
        }
    }

    /// Handle NavigateDown: move selection down within the current page.
    fn navigate_down(&mut self) {
        let visible_count = self.visible_tickets().len();
        if visible_count > 0 && self.selected_row < visible_count - 1 {
            self.selected_row += 1;
        }
    }

    /// Handle PageLeft: go to previous page.
    fn page_left(&mut self) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.selected_row = 0;
        }
    }

    /// Handle PageRight: go to next page.
    fn page_right(&mut self) {
        if self.current_page + 1 < self.total_pages() {
            self.current_page += 1;
            self.selected_row = 0;
        }
    }

    /// Update page size based on the render area height, accounting for
    /// table chrome (header row + top/bottom borders).
    fn update_page_size(&mut self, area_height: u16) {
        // 3 lines of chrome: 1 top border + 1 header row + 1 bottom border
        let chrome = 3u16;
        let available = area_height.saturating_sub(chrome) as usize;
        if available > 0 {
            self.page_size = available;
        }
    }
}

use ratatui::widgets::Widget;

impl Page for TicketsPage {
    fn tab_id(&self) -> TabId {
        TabId::Tickets
    }

    fn title(&self) -> &str {
        "Tickets"
    }

    fn shortcut_char(&self) -> char {
        '1'
    }

    fn handle_action(&mut self, action: Action) -> PageResult {
        match action {
            Action::NavigateUp => {
                self.navigate_up();
                PageResult::Consumed
            }
            Action::NavigateDown => {
                self.navigate_down();
                PageResult::Consumed
            }
            Action::PageLeft => {
                self.page_left();
                PageResult::Consumed
            }
            Action::PageRight => {
                self.page_right();
                PageResult::Consumed
            }
            Action::Quit => PageResult::Quit,
            _ => PageResult::Ignored,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        // Recompute page size from terminal height. Because render takes &self,
        // we cannot mutate here; the caller should call update_page_size before
        // render. However, we still display correctly based on the last-set
        // page_size.
        match &self.data_state {
            DataState::Loading => {
                self.render_message(area, buf, ctx, "Loading...");
            }
            DataState::Error(msg) => {
                self.render_message(area, buf, ctx, &format!("Error: {msg}"));
            }
            DataState::Loaded(_) => {
                let rows = self.build_rows();
                let page_info = format!(
                    " Page {}/{} ({} tickets) ",
                    self.current_page + 1,
                    self.total_pages(),
                    self.ticket_count(),
                );
                let table = ThemedTable {
                    headers: vec!["ID", "Type", "Status", "Priority", "Title"],
                    rows,
                    selected: Some(self.selected_row),
                    widths: vec![
                        Constraint::Length(12),
                        Constraint::Length(8),
                        Constraint::Length(12),
                        Constraint::Length(8),
                        Constraint::Fill(1),
                    ],
                    page_info: Some(page_info),
                };
                table.render(area, buf, ctx);
            }
        }
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "j/k".to_string(),
                description: "Navigate".to_string(),
            },
            FooterCommand {
                key_label: "h/l".to_string(),
                description: "Page".to_string(),
            },
            FooterCommand {
                key_label: "q".to_string(),
                description: "Quit".to_string(),
            },
        ]
    }

    fn on_data(&mut self, payload: &DataPayload) {
        if let DataPayload::Tickets(result) = payload {
            match result {
                Ok(tickets) => {
                    self.data_state = DataState::Loaded(tickets.clone());
                    // Clamp selection and page to valid ranges after data update.
                    self.current_page = 0;
                    self.selected_row = 0;
                }
                Err(msg) => {
                    self.data_state = DataState::Error(msg.clone());
                }
            }
        }
    }

    fn needs_data(&self) -> bool {
        matches!(self.data_state, DataState::Loading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
    }

    #[test]
    fn new_page_needs_data() {
        let page = TicketsPage::new();
        assert!(page.needs_data());
    }

    #[test]
    fn on_data_tickets_ok() {
        let mut page = TicketsPage::new();
        let tickets = vec![make_ticket("t-1", "First"), make_ticket("t-2", "Second")];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));
        assert!(!page.needs_data());
        assert_eq!(page.ticket_count(), 2);
    }

    #[test]
    fn on_data_tickets_error() {
        let mut page = TicketsPage::new();
        page.on_data(&DataPayload::Tickets(Err("connection refused".into())));
        assert!(!page.needs_data());
        assert!(matches!(page.data_state, DataState::Error(_)));
    }

    #[test]
    fn on_data_ignores_flows() {
        let mut page = TicketsPage::new();
        page.on_data(&DataPayload::Flows(Ok(vec![])));
        assert!(page.needs_data()); // still loading
    }

    #[test]
    fn navigate_down_and_up() {
        let mut page = TicketsPage::new();
        let tickets = (0..5)
            .map(|i| make_ticket(&format!("t-{i}"), "T"))
            .collect();
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        assert_eq!(page.selected_row, 0);
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_row, 1);
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_row, 2);
        page.handle_action(Action::NavigateUp);
        assert_eq!(page.selected_row, 1);
    }

    #[test]
    fn navigate_up_does_not_underflow() {
        let mut page = TicketsPage::new();
        let tickets = vec![make_ticket("t-1", "One")];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        page.handle_action(Action::NavigateUp);
        assert_eq!(page.selected_row, 0);
    }

    #[test]
    fn navigate_down_does_not_overflow() {
        let mut page = TicketsPage::new();
        let tickets = vec![make_ticket("t-1", "One"), make_ticket("t-2", "Two")];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        page.handle_action(Action::NavigateDown);
        page.handle_action(Action::NavigateDown);
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_row, 1);
    }

    #[test]
    fn pagination() {
        let mut page = TicketsPage::new();
        page.page_size = 2;
        let tickets = (0..5)
            .map(|i| make_ticket(&format!("t-{i}"), "T"))
            .collect();
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        assert_eq!(page.current_page, 0);
        assert_eq!(page.total_pages(), 3); // 5 tickets / 2 per page = 3 pages

        page.handle_action(Action::PageRight);
        assert_eq!(page.current_page, 1);
        assert_eq!(page.selected_row, 0);

        page.handle_action(Action::PageRight);
        assert_eq!(page.current_page, 2);

        // Can't go past last page
        page.handle_action(Action::PageRight);
        assert_eq!(page.current_page, 2);

        page.handle_action(Action::PageLeft);
        assert_eq!(page.current_page, 1);

        // Can't go before first page
        page.handle_action(Action::PageLeft);
        page.handle_action(Action::PageLeft);
        assert_eq!(page.current_page, 0);
    }

    #[test]
    fn update_page_size_from_area() {
        let mut page = TicketsPage::new();
        // 23 lines total - 3 chrome = 20 rows
        page.update_page_size(23);
        assert_eq!(page.page_size, 20);

        // Small terminal
        page.update_page_size(5);
        assert_eq!(page.page_size, 2);
    }

    #[test]
    fn quit_action_returns_quit() {
        let mut page = TicketsPage::new();
        assert_eq!(page.handle_action(Action::Quit), PageResult::Quit);
    }

    #[test]
    fn unhandled_action_returns_ignored() {
        let mut page = TicketsPage::new();
        assert_eq!(page.handle_action(Action::Select), PageResult::Ignored);
        assert_eq!(page.handle_action(Action::Back), PageResult::Ignored);
    }

    #[test]
    fn tab_id_is_tickets() {
        let page = TicketsPage::new();
        assert_eq!(page.tab_id(), TabId::Tickets);
    }

    #[test]
    fn footer_commands_not_empty() {
        let page = TicketsPage::new();
        let cmds = page.footer_commands();
        assert!(!cmds.is_empty());
    }

    #[test]
    fn visible_tickets_respects_page() {
        let mut page = TicketsPage::new();
        page.page_size = 2;
        let tickets: Vec<_> = (0..5)
            .map(|i| make_ticket(&format!("t-{i}"), &format!("Ticket {i}")))
            .collect();
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        // Page 0: tickets 0, 1
        let visible = page.visible_tickets();
        assert_eq!(visible.len(), 2);
        assert_eq!(visible[0].id, "t-0");

        // Page 2: ticket 4 only
        page.current_page = 2;
        let visible = page.visible_tickets();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].id, "t-4");
    }
}
