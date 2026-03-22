use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use ur_rpc::proto::ticket::Ticket;

use crate::context::TuiContext;
use crate::data::{ActionResult, DataPayload};
use crate::keymap::{Action, Keymap};
use crate::page::{Banner, BannerVariant, FooterCommand, Page, PageResult, StatusMessage, TabId};
use crate::widgets::filter_menu::{FilterMenuResult, FilterMenuState, TicketFilters};
use crate::widgets::priority_picker::{PriorityPickerResult, PriorityPickerState};
use crate::widgets::{MiniProgressBar, ThemedTable};

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

/// Active overlay on this page.
enum Overlay {
    FilterMenu(FilterMenuState),
    PriorityPicker(PriorityPickerState),
}

/// The Tickets tab page.
///
/// Displays all tickets in a themed table with columns: ID, Status, Priority,
/// Progress, Title. Supports row selection (NavigateUp/Down) and client-side
/// pagination (PageLeft/Right). Includes a filter overlay for narrowing results.
pub struct TicketsPage {
    data_state: DataState,
    selected_row: usize,
    current_page: usize,
    page_size: usize,
    overlay: Option<Overlay>,
    filters: TicketFilters,
    /// Cache of filtered + sorted tickets, rebuilt on data or filter change.
    filtered_cache: Vec<Ticket>,
    /// Active notification banner (success/error from async actions).
    active_banner: Option<Banner>,
    /// In-progress status message shown below the tab header.
    active_status: Option<StatusMessage>,
    /// When true, a background refresh is in progress but stale data stays visible.
    refreshing: bool,
}

impl TicketsPage {
    pub fn new() -> Self {
        Self {
            data_state: DataState::Loading,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
            overlay: None,
            filters: TicketFilters::default(),
            filtered_cache: Vec::new(),
            active_banner: None,
            active_status: None,
            refreshing: false,
        }
    }

    /// Total number of pages given the current ticket count and page size.
    fn total_pages(&self) -> usize {
        let count = self.filtered_cache.len();
        if count == 0 || self.page_size == 0 {
            return 1;
        }
        count.div_ceil(self.page_size)
    }

    /// Number of tickets after filtering.
    fn ticket_count(&self) -> usize {
        self.filtered_cache.len()
    }

    /// Returns the slice of tickets visible on the current page.
    fn visible_tickets(&self) -> &[Ticket] {
        let start = self.current_page * self.page_size;
        let end = (start + self.page_size).min(self.filtered_cache.len());
        if start >= self.filtered_cache.len() {
            &[]
        } else {
            &self.filtered_cache[start..end]
        }
    }

    /// Build table row data from visible tickets.
    ///
    /// The Progress column (index 3) is left empty here because the progress
    /// bar is rendered directly to the buffer with themed colors in
    /// [`render_progress_bars`].
    fn build_rows(&self) -> Vec<Vec<String>> {
        self.visible_tickets()
            .iter()
            .map(|t| {
                let status_label = Self::dispatch_label(t);
                vec![
                    t.id.clone(),
                    status_label,
                    t.priority.to_string(),
                    String::new(), // placeholder for progress count
                    String::new(), // placeholder for progress bar
                    t.title.clone(),
                ]
            })
            .collect()
    }

    /// Derive the display label for the Status column from dispatch state.
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
    fn ticket_progress(ticket: &Ticket) -> (u32, u32) {
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
    pub fn update_page_size(&mut self, area_height: u16) {
        // 3 lines of chrome: 1 top border + 1 header row + 1 bottom border
        let chrome = 3u16;
        let available = area_height.saturating_sub(chrome) as usize;
        if available > 0 {
            self.page_size = available;
        }
    }

    /// After loading new data, either clamp the selection (refresh) or reset it (initial load).
    fn update_selection_after_load(&mut self, was_refreshing: bool) {
        if !was_refreshing {
            self.current_page = 0;
            self.selected_row = 0;
            return;
        }
        // Clamp selection to valid ranges without resetting position.
        let visible_count = self.visible_tickets().len();
        if visible_count == 0 {
            self.selected_row = 0;
            self.current_page = 0;
        } else {
            if self.current_page >= self.total_pages() {
                self.current_page = self.total_pages().saturating_sub(1);
            }
            if self.selected_row >= visible_count {
                self.selected_row = visible_count - 1;
            }
        }
    }

    /// Rebuild the filtered + sorted cache from the raw data.
    fn rebuild_cache(&mut self) {
        let tickets = match &self.data_state {
            DataState::Loaded(t) => t,
            _ => {
                self.filtered_cache.clear();
                return;
            }
        };
        self.filtered_cache = tickets
            .iter()
            .filter(|t| self.passes_filter(t))
            .cloned()
            .collect();
        sort_tickets(&mut self.filtered_cache);
    }

    /// Check whether a ticket passes the current filters.
    fn passes_filter(&self, ticket: &Ticket) -> bool {
        // Status filter
        if !self.filters.statuses.is_empty() && !self.filters.statuses.contains(&ticket.status) {
            return false;
        }
        // Priority filter
        if !self.filters.priorities.is_empty()
            && !self.filters.priorities.contains(&ticket.priority)
        {
            return false;
        }
        // Project filter
        if !self.filters.projects.is_empty() && !self.filters.projects.contains(&ticket.project) {
            return false;
        }
        // Show children filter: hide tickets with parent_id when off
        if !self.filters.show_children && !ticket.parent_id.is_empty() {
            return false;
        }
        true
    }

    /// Handle a raw key event when the overlay is active.
    /// Returns `None` if the event was consumed or closed without selection.
    /// Returns `Some((ticket_id, priority))` if the user selected a priority.
    pub fn handle_overlay_key(&mut self, key: KeyEvent) -> Option<(String, i64)> {
        match self.overlay {
            Some(Overlay::FilterMenu(ref mut menu)) => {
                // First check if Esc should collapse submenu before closing
                if matches!(key.code, crossterm::event::KeyCode::Esc) && menu.collapse() {
                    return None;
                }

                let result = menu.handle_key(key, &mut self.filters);
                match result {
                    FilterMenuResult::Consumed => {
                        // Filters may have changed; rebuild cache
                        self.rebuild_cache();
                        self.current_page = 0;
                        self.selected_row = 0;
                    }
                    FilterMenuResult::Close => {
                        self.overlay = None;
                    }
                }
                None
            }
            Some(Overlay::PriorityPicker(ref mut picker)) => {
                let result = picker.handle_key(key);
                match result {
                    PriorityPickerResult::Consumed => None,
                    PriorityPickerResult::Close => {
                        self.overlay = None;
                        None
                    }
                    PriorityPickerResult::Selected(priority) => {
                        let ticket_id = self.selected_ticket_id();
                        self.overlay = None;
                        ticket_id.map(|id| (id, priority))
                    }
                }
            }
            None => None,
        }
    }

    /// Returns true if an overlay is currently active.
    pub fn has_overlay(&self) -> bool {
        self.overlay.is_some()
    }

    /// Close any active overlay.
    pub fn close_overlay(&mut self) {
        self.overlay = None;
    }

    /// Returns the ticket ID of the currently selected row, if any.
    pub fn selected_ticket_id(&self) -> Option<String> {
        let visible = self.visible_tickets();
        visible.get(self.selected_row).map(|t| t.id.clone())
    }

    /// Set an in-progress status message (e.g., for dispatch).
    pub fn set_status(&mut self, text: String) {
        self.active_status = Some(StatusMessage {
            text,
            dismissable: true,
        });
    }

    /// Handle an async action result by showing a success or error banner.
    pub fn on_action_result(&mut self, result: &ActionResult) {
        // Clear in-progress status before showing banner.
        self.active_status = None;
        match &result.result {
            Ok(msg) => {
                if !result.silent_on_success {
                    self.active_banner = Some(Banner {
                        message: msg.clone(),
                        variant: BannerVariant::Success,
                        created_at: Instant::now(),
                    });
                }
            }
            Err(msg) => {
                self.active_banner = Some(Banner {
                    message: msg.clone(),
                    variant: BannerVariant::Error,
                    created_at: Instant::now(),
                });
            }
        }
    }
}

/// Sort tickets: priority ascending (P0 first), tickets with children rank
/// higher than same-priority leaves.
fn sort_tickets(tickets: &mut [Ticket]) {
    tickets.sort_by(|a, b| {
        // Primary sort: priority ascending
        let prio = a.priority.cmp(&b.priority);
        if prio != std::cmp::Ordering::Equal {
            return prio;
        }
        // Secondary: tickets with children before leaves at same priority
        let a_has_children = a.children_total > 0;
        let b_has_children = b.children_total > 0;
        b_has_children.cmp(&a_has_children)
    });
}

/// The column index of the progress count label in the table.
const PROGRESS_COUNT_COL: usize = 3;
/// The column index of the progress bar in the table.
const PROGRESS_BAR_COL: usize = 4;

/// Render mini progress bars and count labels over the placeholder columns.
///
/// Calculates each row's progress column rects by resolving the table layout
/// constraints, then renders bar and count separately for consistent bar width.
fn render_progress_bars(
    page: &TicketsPage,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    widths: &[Constraint],
) {
    use ratatui::layout::Layout;

    // The table block has a 1-cell border on each side.
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1, // skip top border
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    // Resolve column positions using the same constraints as the table.
    let col_areas = Layout::horizontal(widths.to_vec()).split(inner);
    let bar_area = col_areas.get(PROGRESS_BAR_COL);
    let count_area = col_areas.get(PROGRESS_COUNT_COL);

    if bar_area.is_none() && count_area.is_none() {
        return;
    }

    // First row inside inner is the header; data rows start at inner.y + 1.
    let data_start_y = inner.y + 1;
    let visible = page.visible_tickets();

    for (i, ticket) in visible.iter().enumerate() {
        let row_y = data_start_y + i as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let (completed, total) = TicketsPage::ticket_progress(ticket);
        let is_selected = i == page.selected_row;
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

use ratatui::widgets::Widget;

impl Page for TicketsPage {
    fn tab_id(&self) -> TabId {
        TabId::Tickets
    }

    fn title(&self) -> &str {
        "Tickets"
    }

    fn shortcut_char(&self) -> char {
        't'
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
            Action::Refresh => {
                self.refreshing = true;
                self.active_status = Some(StatusMessage {
                    text: "Refreshing tickets...".to_string(),
                    dismissable: true,
                });
                PageResult::Consumed
            }
            Action::Filter => {
                // Open filter menu — pass empty projects, will be overridden
                // at render time via ctx, but state is initialized here.
                PageResult::Consumed
            }
            Action::Quit => PageResult::Quit,
            _ => PageResult::Ignored,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
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
                let widths = vec![
                    Constraint::Length(12),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(10),
                    Constraint::Fill(1),
                ];
                let table = ThemedTable {
                    headers: vec!["ID", "Status", "P", "Progress", "", "Title"],
                    rows,
                    selected: Some(self.selected_row),
                    widths: widths.clone(),
                    page_info: Some(page_info),
                };
                table.render(area, buf, ctx);

                // Render progress bars on top of the placeholder cells.
                render_progress_bars(self, area, buf, ctx, &widths);
            }
        }

        // Render overlay on top
        match &self.overlay {
            Some(Overlay::FilterMenu(menu)) => {
                menu.render(area, buf, ctx, &self.filters);
            }
            Some(Overlay::PriorityPicker(picker)) => {
                picker.render(area, buf, ctx);
            }
            None => {}
        }
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        match &self.overlay {
            Some(Overlay::FilterMenu(menu)) => return menu.footer_commands(),
            Some(Overlay::PriorityPicker(picker)) => return picker.footer_commands(),
            None => {}
        }
        vec![
            FooterCommand {
                key_label: keymap.label_for(&Action::Filter),
                description: "Filter".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::SetPriority),
                description: "Priority".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Dispatch),
                description: "Dispatch".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::CloseTicket),
                description: "Close".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::OpenTicket),
                description: "Open".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::NavigateDown),
                description: "Down".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::NavigateUp),
                description: "Up".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.combined_label(&Action::PageLeft, &Action::PageRight),
                description: "Page".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Refresh),
                description: "Refresh".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Back),
                description: "Back".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::OpenSettings),
                description: "Settings".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Quit),
                description: "Quit".to_string(),
                common: true,
            },
        ]
    }

    fn on_data(&mut self, payload: &DataPayload) {
        if let DataPayload::Tickets(result) = payload {
            let was_refreshing = self.refreshing;
            self.refreshing = false;
            self.active_status = None;
            match result {
                Ok(tickets) => {
                    self.data_state = DataState::Loaded(tickets.clone());
                    self.rebuild_cache();
                    self.update_selection_after_load(was_refreshing);
                }
                Err(msg) => {
                    self.data_state = DataState::Error(msg.clone());
                    self.filtered_cache.clear();
                }
            }
        }
    }

    fn needs_data(&self) -> bool {
        matches!(self.data_state, DataState::Loading) || self.refreshing
    }

    fn banner(&self) -> Option<&Banner> {
        self.active_banner.as_ref()
    }

    fn dismiss_banner(&mut self) {
        self.active_banner = None;
    }

    fn tick_banner(&mut self) {
        if let Some(ref banner) = self.active_banner
            && banner.is_expired()
        {
            self.active_banner = None;
        }
    }

    fn status(&self) -> Option<&StatusMessage> {
        self.active_status.as_ref()
    }

    fn dismiss_status(&mut self) {
        self.active_status = None;
    }

    fn clear_status(&mut self) {
        self.active_status = None;
    }

    fn mark_stale(&mut self) {
        self.data_state = DataState::Loading;
    }
}

/// Open the filter menu overlay on the tickets page.
pub fn open_filter_menu(page: &mut TicketsPage, projects: &[String]) {
    page.overlay = Some(Overlay::FilterMenu(FilterMenuState::new(projects.to_vec())));
}

/// Open the priority picker overlay on the tickets page, initialized to the
/// selected ticket's current priority.
pub fn open_priority_picker(page: &mut TicketsPage) {
    let current_priority = page
        .visible_tickets()
        .get(page.selected_row)
        .map(|t| t.priority)
        .unwrap_or(2);
    page.overlay = Some(Overlay::PriorityPicker(PriorityPickerState::new(
        current_priority,
    )));
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
            children_total: 0,
            children_completed: 0,
            dispatch_status: String::new(),
        }
    }

    fn make_ticket_with_fields(
        id: &str,
        status: &str,
        priority: i64,
        project: &str,
        parent_id: &str,
        children_total: i32,
    ) -> Ticket {
        Ticket {
            id: id.to_string(),
            ticket_type: "task".to_string(),
            status: status.to_string(),
            priority,
            parent_id: parent_id.to_string(),
            title: format!("Ticket {id}"),
            body: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            project: project.to_string(),
            branch: String::new(),
            depth: 0,
            children_total,
            children_completed: 0,
            dispatch_status: String::new(),
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
        let keymap = Keymap::default();
        let cmds = page.footer_commands(&keymap);
        assert!(!cmds.is_empty());
    }

    #[test]
    fn refresh_keeps_data_visible() {
        let mut page = TicketsPage::new();
        let tickets = vec![make_ticket("t-1", "First")];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));
        assert!(!page.needs_data());

        let result = page.handle_action(Action::Refresh);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.needs_data());
        assert!(page.refreshing);
        // Data state is still Loaded, not Loading — stale data stays visible.
        assert!(matches!(page.data_state, DataState::Loaded(_)));
    }

    #[test]
    fn visible_tickets_respects_page() {
        let mut page = TicketsPage::new();
        page.page_size = 2;
        let tickets: Vec<_> = (0..5)
            .map(|i| make_ticket(&format!("t-{i}"), &format!("Ticket {i}")))
            .collect();
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        // Page 0: first 2 tickets
        let visible = page.visible_tickets();
        assert_eq!(visible.len(), 2);

        // Page 2: last ticket only
        page.current_page = 2;
        let visible = page.visible_tickets();
        assert_eq!(visible.len(), 1);
    }

    #[test]
    fn default_filter_hides_children() {
        let mut page = TicketsPage::new();
        let tickets = vec![
            make_ticket_with_fields("t-1", "open", 2, "test", "", 0),
            make_ticket_with_fields("t-2", "open", 2, "test", "t-1", 0), // child
        ];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        // Default: show_children=off, so child is hidden
        assert_eq!(page.ticket_count(), 1);
        assert_eq!(page.filtered_cache[0].id, "t-1");
    }

    #[test]
    fn filter_by_status() {
        let mut page = TicketsPage::new();
        let tickets = vec![
            make_ticket_with_fields("t-1", "open", 2, "test", "", 0),
            make_ticket_with_fields("t-2", "closed", 2, "test", "", 0),
            make_ticket_with_fields("t-3", "in_progress", 2, "test", "", 0),
        ];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        // Default: status=open,in_progress
        assert_eq!(page.ticket_count(), 2);

        // Show all statuses
        page.filters.statuses.clear();
        page.rebuild_cache();
        assert_eq!(page.ticket_count(), 3);
    }

    #[test]
    fn filter_by_priority() {
        let mut page = TicketsPage::new();
        page.filters.statuses.clear(); // Show all statuses
        let tickets = vec![
            make_ticket_with_fields("t-1", "open", 0, "test", "", 0),
            make_ticket_with_fields("t-2", "open", 2, "test", "", 0),
            make_ticket_with_fields("t-3", "open", 4, "test", "", 0),
        ];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        // Filter to P0 only
        page.filters.priorities = vec![0];
        page.rebuild_cache();
        assert_eq!(page.ticket_count(), 1);
        assert_eq!(page.filtered_cache[0].id, "t-1");
    }

    #[test]
    fn filter_by_project() {
        let mut page = TicketsPage::new();
        page.filters.statuses.clear();
        let tickets = vec![
            make_ticket_with_fields("t-1", "open", 2, "alpha", "", 0),
            make_ticket_with_fields("t-2", "open", 2, "beta", "", 0),
        ];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        page.filters.projects = vec!["alpha".to_string()];
        page.rebuild_cache();
        assert_eq!(page.ticket_count(), 1);
        assert_eq!(page.filtered_cache[0].project, "alpha");
    }

    #[test]
    fn sorting_priority_ascending() {
        let mut page = TicketsPage::new();
        page.filters.statuses.clear();
        let tickets = vec![
            make_ticket_with_fields("t-3", "open", 4, "test", "", 0),
            make_ticket_with_fields("t-1", "open", 0, "test", "", 0),
            make_ticket_with_fields("t-2", "open", 2, "test", "", 0),
        ];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        assert_eq!(page.filtered_cache[0].id, "t-1"); // P0
        assert_eq!(page.filtered_cache[1].id, "t-2"); // P2
        assert_eq!(page.filtered_cache[2].id, "t-3"); // P4
    }

    #[test]
    fn sorting_children_rank_higher() {
        let mut page = TicketsPage::new();
        page.filters.statuses.clear();
        let tickets = vec![
            make_ticket_with_fields("t-leaf", "open", 2, "test", "", 0),
            make_ticket_with_fields("t-parent", "open", 2, "test", "", 3),
        ];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        // Parent (has children) should come before leaf at same priority
        assert_eq!(page.filtered_cache[0].id, "t-parent");
        assert_eq!(page.filtered_cache[1].id, "t-leaf");
    }

    #[test]
    fn filters_persist_across_refresh() {
        let mut page = TicketsPage::new();
        page.filters.priorities = vec![0, 1];

        let tickets = vec![make_ticket("t-1", "First")];
        page.on_data(&DataPayload::Tickets(Ok(tickets)));

        // Refresh
        page.handle_action(Action::Refresh);
        assert!(page.needs_data());

        // Reload data — filters should still be set
        let tickets2 = vec![make_ticket("t-2", "Second")];
        page.on_data(&DataPayload::Tickets(Ok(tickets2)));
        assert_eq!(page.filters.priorities, vec![0, 1]);
    }

    #[test]
    fn open_filter_menu_creates_overlay() {
        let mut page = TicketsPage::new();
        assert!(!page.has_overlay());

        open_filter_menu(&mut page, &["proj1".to_string()]);
        assert!(page.has_overlay());
    }

    #[test]
    fn overlay_footer_differs() {
        let mut page = TicketsPage::new();
        let keymap = Keymap::default();
        let normal_cmds = page.footer_commands(&keymap);

        open_filter_menu(&mut page, &[]);
        let overlay_cmds = page.footer_commands(&keymap);

        // Footer should be different when overlay is open
        assert_ne!(normal_cmds.len(), overlay_cmds.len());
    }
}
