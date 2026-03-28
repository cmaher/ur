use std::time::Instant;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use tracing::{debug, warn};
use ur_rpc::proto::ticket::Ticket;

use crate::context::TuiContext;
use crate::data::{ActionResult, DataPayload};
use crate::keymap::{Action, Keymap};
use crate::page::{Banner, BannerVariant, FooterCommand, StatusMessage};
use crate::pages::ticket_detail::TicketDetailScreen;
use crate::screen::{Screen, ScreenResult};
use crate::widgets::filter_menu::{FilterMenuResult, FilterMenuState, TicketFilters};
use crate::widgets::force_close_confirm::{ForceCloseConfirmResult, ForceCloseConfirmState};
use crate::widgets::priority_picker::{PriorityPickerResult, PriorityPickerState};
use crate::widgets::{MiniProgressBar, ThemedTable};

/// Internal state for the ticket data lifecycle.
#[derive(Debug, Clone)]
enum DataState {
    /// No data has been fetched yet.
    Loading,
    /// Data was fetched successfully. Stores only the current page of tickets.
    Loaded(Vec<Ticket>),
    /// Data fetch failed with the given error message.
    Error(String),
}

/// Active overlay on this page.
enum Overlay {
    FilterMenu(FilterMenuState),
    PriorityPicker(PriorityPickerState),
    ForceCloseConfirm(ForceCloseConfirmState),
}

/// Result from handling an overlay key event.
pub enum OverlayAction {
    /// No action needed by the caller.
    None,
    /// User selected a priority for the given ticket.
    SetPriority { ticket_id: String, priority: i64 },
    /// User confirmed force-closing the given ticket.
    ForceClose { ticket_id: String },
}

/// Pagination parameters for server-side ticket fetching.
pub struct PaginationParams {
    pub page_size: i32,
    pub offset: i32,
    pub include_children: bool,
}

/// The Tickets tab list screen.
///
/// Displays tickets in a themed table with columns: ID, Status, Priority,
/// Progress, Title. Supports row selection (NavigateUp/Down) and server-side
/// pagination (PageLeft/Right). Includes a filter overlay for narrowing results.
pub struct TicketsListScreen {
    data_state: DataState,
    selected_row: usize,
    current_page: usize,
    page_size: usize,
    /// Server-provided total count of tickets matching the current query.
    total_count: i32,
    overlay: Option<Overlay>,
    filters: TicketFilters,
    /// Active notification banner (success/error from async actions).
    active_banner: Option<Banner>,
    /// In-progress status message shown below the tab header.
    active_status: Option<StatusMessage>,
    /// Ticket ID to navigate to (push detail) on the next data cycle.
    pending_goto: Option<String>,
    /// Ticket ID to highlight (select without pushing) on the next data cycle.
    pending_highlight: Option<String>,
}

impl TicketsListScreen {
    pub fn new(filter_config: &ur_config::TicketFilterConfig) -> Self {
        Self {
            data_state: DataState::Loading,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
            total_count: 0,
            overlay: None,
            filters: TicketFilters::from_config(filter_config),
            active_banner: None,
            active_status: None,
            pending_goto: None,
            pending_highlight: None,
        }
    }

    /// Returns a reference to the current filters for persistence.
    pub fn filters(&self) -> &TicketFilters {
        &self.filters
    }

    /// Returns the pending goto ticket ID, if any.
    pub fn pending_goto(&self) -> Option<&str> {
        self.pending_goto.as_deref()
    }

    /// Returns the pending highlight ticket ID, if any.
    pub fn pending_highlight(&self) -> Option<&str> {
        self.pending_highlight.as_deref()
    }

    /// Returns the current pagination parameters for server-side fetching.
    pub fn pagination_params(&self) -> PaginationParams {
        let offset = self.current_page * self.page_size;
        PaginationParams {
            page_size: self.page_size as i32,
            offset: offset as i32,
            include_children: self.filters.show_children,
        }
    }

    /// Total number of pages given the server-provided total_count and page size.
    fn total_pages(&self) -> usize {
        let count = self.total_count as usize;
        if count == 0 || self.page_size == 0 {
            return 1;
        }
        count.div_ceil(self.page_size)
    }

    /// Total ticket count from the server.
    fn ticket_count(&self) -> i32 {
        self.total_count
    }

    /// Returns the tickets on the current page (the entire loaded set).
    fn visible_tickets(&self) -> &[Ticket] {
        match &self.data_state {
            DataState::Loaded(tickets) => tickets,
            _ => &[],
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

    /// Handle PageLeft: go to previous page via server fetch.
    fn page_left(&mut self) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.selected_row = 0;
            self.mark_stale();
        }
    }

    /// Handle PageRight: go to next page via server fetch.
    fn page_right(&mut self) {
        if self.current_page + 1 < self.total_pages() {
            self.current_page += 1;
            self.selected_row = 0;
            self.mark_stale();
        }
    }

    /// Update page size based on the render area height, accounting for
    /// table chrome (header row + top/bottom borders).
    ///
    /// If the page size changes, resets to page 0 and triggers a re-fetch.
    pub fn update_page_size(&mut self, area_height: u16) {
        // 3 lines of chrome: 1 top border + 1 header row + 1 bottom border
        let chrome = 3u16;
        let available = area_height.saturating_sub(chrome) as usize;
        if available > 0 && available != self.page_size {
            self.page_size = available;
            self.current_page = 0;
            self.selected_row = 0;
            self.mark_stale();
        }
    }

    /// Apply a server page result, clamping selection to the new data range.
    fn apply_page_result(&mut self, result: &Result<(Vec<Ticket>, i32), String>) {
        match result {
            Ok((tickets, total_count)) => {
                self.total_count = *total_count;
                // Clamp to last valid page if offset is past end.
                let max_page = self.total_pages().saturating_sub(1);
                if self.total_count > 0 && self.current_page > max_page {
                    warn!(
                        current_page = self.current_page,
                        max_page = max_page,
                        "tickets: stale-reclamp, current_page > max_page; marking stale for re-fetch"
                    );
                    self.current_page = max_page;
                    // Need another fetch at the clamped offset.
                    self.mark_stale();
                    return;
                }
                debug!(
                    count = tickets.len(),
                    total_count = total_count,
                    "tickets: Loading -> Loaded"
                );
                self.data_state = DataState::Loaded(tickets.clone());
                self.clamp_selection();
            }
            Err(msg) => {
                debug!(error = %msg, "tickets: Loading -> Error");
                self.data_state = DataState::Error(msg.clone());
                self.total_count = 0;
            }
        }
    }

    /// Clamp selection to valid range within the current page.
    fn clamp_selection(&mut self) {
        let visible_count = self.visible_tickets().len();
        if visible_count == 0 {
            self.selected_row = 0;
        } else if self.selected_row >= visible_count {
            self.selected_row = visible_count.saturating_sub(1);
        }
    }

    /// Handle a raw key event when the overlay is active.
    /// Returns an `OverlayAction` indicating what the caller should do.
    pub fn handle_overlay_key(&mut self, key: KeyEvent) -> OverlayAction {
        match self.overlay {
            Some(Overlay::FilterMenu(ref mut menu)) => {
                // First check if Esc should collapse submenu before closing
                if matches!(key.code, crossterm::event::KeyCode::Esc) && menu.collapse() {
                    return OverlayAction::None;
                }

                let result = menu.handle_key(key, &mut self.filters);
                match result {
                    FilterMenuResult::Consumed => {
                        // Filters changed; reset to page 0 and re-fetch from server.
                        self.current_page = 0;
                        self.selected_row = 0;
                        self.mark_stale();
                    }
                    FilterMenuResult::Close => {
                        self.overlay = None;
                    }
                }
                OverlayAction::None
            }
            Some(Overlay::PriorityPicker(ref mut picker)) => {
                let result = picker.handle_key(key);
                match result {
                    PriorityPickerResult::Consumed => OverlayAction::None,
                    PriorityPickerResult::Close => {
                        self.overlay = None;
                        OverlayAction::None
                    }
                    PriorityPickerResult::Selected(priority) => {
                        let ticket_id = self.selected_ticket_id();
                        self.overlay = None;
                        match ticket_id {
                            Some(id) => OverlayAction::SetPriority {
                                ticket_id: id,
                                priority,
                            },
                            None => OverlayAction::None,
                        }
                    }
                }
            }
            Some(Overlay::ForceCloseConfirm(ref mut state)) => {
                let result = state.handle_key(key);
                match result {
                    ForceCloseConfirmResult::Consumed => OverlayAction::None,
                    ForceCloseConfirmResult::Cancelled => {
                        self.overlay = None;
                        OverlayAction::None
                    }
                    ForceCloseConfirmResult::Confirmed => {
                        let ticket_id = state.ticket_id.clone();
                        self.overlay = None;
                        OverlayAction::ForceClose { ticket_id }
                    }
                }
            }
            None => OverlayAction::None,
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
        self.visible_tickets()
            .get(self.selected_row)
            .map(|t| t.id.clone())
    }

    /// Returns the single active project filter, if exactly one project is selected.
    pub fn single_project_filter(&self) -> Option<&str> {
        if self.filters.projects.len() == 1 {
            Some(&self.filters.projects[0])
        } else {
            None
        }
    }

    /// Returns a reference to the currently selected ticket, if any.
    pub fn selected_ticket(&self) -> Option<&Ticket> {
        self.visible_tickets().get(self.selected_row)
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

/// The column index of the progress count label in the table.
const PROGRESS_COUNT_COL: usize = 3;
/// The column index of the progress bar in the table.
const PROGRESS_BAR_COL: usize = 4;

/// Render mini progress bars and count labels over the placeholder columns.
///
/// Calculates each row's progress column rects by resolving the table layout
/// constraints, then renders bar and count separately for consistent bar width.
fn render_progress_bars(
    page: &TicketsListScreen,
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

        let (completed, total) = TicketsListScreen::ticket_progress(ticket);
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

impl TicketsListScreen {
    /// Display title shown in the header tab bar.
    pub fn title(&self) -> &str {
        "Tickets"
    }

    /// The character used as a keyboard shortcut to switch to this tab.
    pub fn shortcut_char(&self) -> char {
        't'
    }
}

impl Screen for TicketsListScreen {
    fn handle_action(&mut self, action: Action) -> ScreenResult {
        match action {
            Action::NavigateUp => {
                self.navigate_up();
                ScreenResult::Consumed
            }
            Action::NavigateDown => {
                self.navigate_down();
                ScreenResult::Consumed
            }
            Action::PageLeft => {
                self.page_left();
                ScreenResult::Consumed
            }
            Action::PageRight => {
                self.page_right();
                ScreenResult::Consumed
            }
            Action::Refresh => {
                self.mark_stale();
                self.active_status = Some(StatusMessage {
                    text: "Refreshing tickets...".to_string(),
                    dismissable: true,
                });
                ScreenResult::Consumed
            }
            Action::Filter => {
                // Open filter menu — pass empty projects, will be overridden
                // at render time via ctx, but state is initialized here.
                ScreenResult::Consumed
            }
            Action::Select => {
                if let Some(ticket) = self.selected_ticket() {
                    let detail = TicketDetailScreen::new(ticket.id.clone(), ticket.project.clone());
                    ScreenResult::Push(Box::new(detail))
                } else {
                    ScreenResult::Consumed
                }
            }
            Action::Quit => ScreenResult::Quit,
            _ => ScreenResult::Ignored,
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
            Some(Overlay::ForceCloseConfirm(state)) => {
                state.render(area, buf, ctx);
            }
            None => {}
        }
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        match &self.overlay {
            Some(Overlay::FilterMenu(menu)) => return menu.footer_commands(),
            Some(Overlay::PriorityPicker(picker)) => return picker.footer_commands(),
            Some(Overlay::ForceCloseConfirm(state)) => return state.footer_commands(),
            None => {}
        }
        vec![
            FooterCommand {
                key_label: keymap.label_for(&Action::CreateTicket),
                description: "Create".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Dispatch),
                description: "Dispatch".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::OpenTicket),
                description: "Open".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::SetPriority),
                description: "Priority".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::LaunchDesign),
                description: "Design".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::CloseTicket),
                description: "Close".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Filter),
                description: "Filter".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Select),
                description: "Select".to_string(),
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
        match payload {
            DataPayload::Tickets(Ok((tickets, total_count))) => {
                self.active_status = None;
                self.apply_page_result(&Ok((tickets.clone(), *total_count)));
            }
            DataPayload::Tickets(Err(msg)) => {
                self.active_status = None;
                self.apply_page_result(&Err(msg.clone()));
            }
            _ => {}
        }
    }

    fn needs_data(&self) -> bool {
        matches!(self.data_state, DataState::Loading)
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

    fn set_status(&mut self, text: String) {
        self.active_status = Some(StatusMessage {
            text,
            dismissable: true,
        });
    }

    fn dismiss_status(&mut self) {
        self.active_status = None;
    }

    fn clear_status(&mut self) {
        self.active_status = None;
    }

    fn mark_stale(&mut self) {
        debug!("tickets: mark_stale");
        self.data_state = DataState::Loading;
    }

    fn as_any_tickets(&self) -> Option<&crate::pages::TicketsListScreen> {
        Some(self)
    }

    fn as_any_tickets_mut(&mut self) -> Option<&mut crate::pages::TicketsListScreen> {
        Some(self)
    }

    fn set_pending_goto(&mut self, ticket_id: String) {
        self.pending_goto = Some(ticket_id);
    }

    fn set_pending_highlight(&mut self, id: String) {
        self.pending_highlight = Some(id);
    }
}

/// Open the filter menu overlay on the tickets list screen.
pub fn open_filter_menu(page: &mut TicketsListScreen, projects: &[String]) {
    page.overlay = Some(Overlay::FilterMenu(FilterMenuState::new(projects.to_vec())));
}

/// Open the force-close confirmation overlay on the tickets list screen.
pub fn open_force_close_confirm(
    page: &mut TicketsListScreen,
    ticket_id: String,
    open_children: i32,
) {
    page.overlay = Some(Overlay::ForceCloseConfirm(ForceCloseConfirmState {
        ticket_id,
        open_children,
    }));
}

/// Open the priority picker overlay on the tickets list screen, initialized to the
/// selected ticket's current priority.
pub fn open_priority_picker(page: &mut TicketsListScreen) {
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
        let page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        assert!(page.needs_data());
    }

    #[test]
    fn on_data_tickets_ok() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let tickets = vec![make_ticket("t-1", "First"), make_ticket("t-2", "Second")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 2))));
        assert!(!page.needs_data());
        assert_eq!(page.ticket_count(), 2);
    }

    #[test]
    fn on_data_tickets_error() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        page.on_data(&DataPayload::Tickets(Err("connection refused".into())));
        assert!(!page.needs_data());
        assert!(matches!(page.data_state, DataState::Error(_)));
    }

    #[test]
    fn on_data_ignores_flows() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        page.on_data(&DataPayload::Flows(Ok((vec![], 0))));
        assert!(page.needs_data()); // still loading
    }

    #[test]
    fn navigate_down_and_up() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let tickets = (0..5)
            .map(|i| make_ticket(&format!("t-{i}"), "T"))
            .collect();
        page.on_data(&DataPayload::Tickets(Ok((tickets, 5))));

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
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let tickets = vec![make_ticket("t-1", "One")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 1))));

        page.handle_action(Action::NavigateUp);
        assert_eq!(page.selected_row, 0);
    }

    #[test]
    fn navigate_down_does_not_overflow() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let tickets = vec![make_ticket("t-1", "One"), make_ticket("t-2", "Two")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 2))));

        page.handle_action(Action::NavigateDown);
        page.handle_action(Action::NavigateDown);
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_row, 1);
    }

    #[test]
    fn server_side_pagination() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        page.page_size = 2;
        // Server returns page 0 with 2 tickets, total_count=5
        let tickets = vec![make_ticket("t-0", "T"), make_ticket("t-1", "T")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 5))));

        assert_eq!(page.current_page, 0);
        assert_eq!(page.total_pages(), 3); // 5 tickets / 2 per page = 3 pages

        // PageRight triggers server fetch (marks stale)
        page.handle_action(Action::PageRight);
        assert_eq!(page.current_page, 1);
        assert_eq!(page.selected_row, 0);
        assert!(page.needs_data()); // marked stale for server fetch

        // Simulate server response for page 1
        let tickets_p1 = vec![make_ticket("t-2", "T"), make_ticket("t-3", "T")];
        page.on_data(&DataPayload::Tickets(Ok((tickets_p1, 5))));
        assert!(!page.needs_data());

        page.handle_action(Action::PageRight);
        assert_eq!(page.current_page, 2);
        assert!(page.needs_data());

        // Simulate fetch complete
        let tickets_p2 = vec![make_ticket("t-4", "T")];
        page.on_data(&DataPayload::Tickets(Ok((tickets_p2, 5))));

        // Can't go past last page
        page.handle_action(Action::PageRight);
        assert_eq!(page.current_page, 2);

        page.handle_action(Action::PageLeft);
        assert_eq!(page.current_page, 1);

        // Simulate fetch complete
        let tickets_p1b = vec![make_ticket("t-2", "T"), make_ticket("t-3", "T")];
        page.on_data(&DataPayload::Tickets(Ok((tickets_p1b, 5))));

        // Can't go before first page
        page.handle_action(Action::PageLeft);
        assert_eq!(page.current_page, 0);
        let tickets_p0b = vec![make_ticket("t-0", "T"), make_ticket("t-1", "T")];
        page.on_data(&DataPayload::Tickets(Ok((tickets_p0b, 5))));
        page.handle_action(Action::PageLeft);
        assert_eq!(page.current_page, 0);
    }

    #[test]
    fn update_page_size_resets_and_refetches() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let tickets = vec![make_ticket("t-1", "A")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 10))));
        page.current_page = 2;

        // Change page size triggers reset to page 0 and re-fetch
        page.update_page_size(13); // 13 - 3 chrome = 10
        assert_eq!(page.page_size, 10);
        assert_eq!(page.current_page, 0);
        assert!(page.needs_data());
    }

    #[test]
    fn update_page_size_no_change_no_refetch() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        // Default page_size is 20, so area_height=23 gives 20
        page.update_page_size(23);
        assert_eq!(page.page_size, 20);
        // Load data so page is no longer stale
        let tickets = vec![make_ticket("t-1", "A")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 1))));

        // Same size again should not trigger re-fetch
        page.update_page_size(23);
        assert!(!page.needs_data());
    }

    #[test]
    fn quit_action_returns_quit() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        assert!(matches!(
            page.handle_action(Action::Quit),
            ScreenResult::Quit
        ));
    }

    #[test]
    fn select_action_returns_consumed() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        assert!(matches!(
            page.handle_action(Action::Select),
            ScreenResult::Consumed
        ));
    }

    #[test]
    fn unhandled_action_returns_ignored() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        assert!(matches!(
            page.handle_action(Action::Back),
            ScreenResult::Ignored
        ));
    }

    #[test]
    fn footer_commands_not_empty() {
        let page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let keymap = Keymap::default();
        let cmds = page.footer_commands(&keymap);
        assert!(!cmds.is_empty());
    }

    #[test]
    fn refresh_marks_stale_for_refetch() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let tickets = vec![make_ticket("t-1", "First")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 1))));
        assert!(!page.needs_data());

        let result = page.handle_action(Action::Refresh);
        assert!(matches!(result, ScreenResult::Consumed));
        assert!(page.needs_data());
    }

    #[test]
    fn visible_tickets_returns_current_page() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let tickets = vec![make_ticket("t-0", "A"), make_ticket("t-1", "B")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 5))));

        // All tickets from server response are visible (they are the current page)
        let visible = page.visible_tickets();
        assert_eq!(visible.len(), 2);
    }

    #[test]
    fn page_indicator_uses_total_count() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        page.page_size = 2;
        let tickets = vec![make_ticket("t-0", "A"), make_ticket("t-1", "B")];
        // Server says total_count=7, so 4 pages
        page.on_data(&DataPayload::Tickets(Ok((tickets, 7))));
        assert_eq!(page.total_pages(), 4);
        assert_eq!(page.ticket_count(), 7);
    }

    #[test]
    fn offset_past_end_clamps_to_last_page() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        page.page_size = 2;
        page.current_page = 5; // Way past end
        // Server says total_count=4 (2 pages)
        let tickets = vec![];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 4))));
        // Should clamp to last valid page and trigger re-fetch
        assert_eq!(page.current_page, 1);
        assert!(page.needs_data());
    }

    #[test]
    fn include_children_in_pagination_params() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        // Default: show_children=false
        let params = page.pagination_params();
        assert!(!params.include_children);

        page.filters.show_children = true;
        let params = page.pagination_params();
        assert!(params.include_children);
    }

    #[test]
    fn pagination_params_reflect_current_state() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        page.page_size = 10;
        page.current_page = 3;
        let params = page.pagination_params();
        assert_eq!(params.page_size, 10);
        assert_eq!(params.offset, 30);
    }

    #[test]
    fn open_filter_menu_creates_overlay() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        assert!(!page.has_overlay());

        open_filter_menu(&mut page, &["proj1".to_string()]);
        assert!(page.has_overlay());
    }

    #[test]
    fn overlay_footer_differs() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let keymap = Keymap::default();
        let normal_cmds = page.footer_commands(&keymap);

        open_filter_menu(&mut page, &[]);
        let overlay_cmds = page.footer_commands(&keymap);

        // Footer should be different when overlay is open
        assert_ne!(normal_cmds.len(), overlay_cmds.len());
    }

    #[test]
    fn full_list_load_replaces_page() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let batch1 = vec![make_ticket("t-1", "A"), make_ticket("t-2", "B")];
        page.on_data(&DataPayload::Tickets(Ok((batch1, 2))));
        assert_eq!(page.visible_tickets().len(), 2);

        // Full list load with different tickets replaces all
        let batch2 = vec![make_ticket("t-3", "C")];
        page.on_data(&DataPayload::Tickets(Ok((batch2, 1))));
        assert_eq!(page.visible_tickets().len(), 1);
        assert_eq!(page.visible_tickets()[0].id, "t-3");
    }

    #[test]
    fn filters_persist_across_refresh() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        page.filters.show_children = true;

        let tickets = vec![make_ticket("t-1", "First")];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 1))));

        // Refresh
        page.handle_action(Action::Refresh);
        assert!(page.needs_data());

        // Reload data — filters should still be set
        let tickets2 = vec![make_ticket("t-2", "Second")];
        page.on_data(&DataPayload::Tickets(Ok((tickets2, 1))));
        assert!(page.filters.show_children);
    }

    #[test]
    fn selection_clamped_when_page_shrinks() {
        let mut page = TicketsListScreen::new(&ur_config::TicketFilterConfig::default());
        let tickets = vec![
            make_ticket("t-1", "A"),
            make_ticket("t-2", "B"),
            make_ticket("t-3", "C"),
        ];
        page.on_data(&DataPayload::Tickets(Ok((tickets, 3))));

        // Select third row
        page.handle_action(Action::NavigateDown);
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_row, 2);

        // Load fewer tickets — selection should be clamped
        page.mark_stale();
        let tickets2 = vec![make_ticket("t-1", "A")];
        page.on_data(&DataPayload::Tickets(Ok((tickets2, 1))));
        assert_eq!(page.selected_row, 0);
    }
}
