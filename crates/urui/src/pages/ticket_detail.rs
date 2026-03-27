use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use tracing::debug;
use ur_markdown::{MarkdownColors, render_markdown};
use ur_rpc::proto::ticket::{GetTicketResponse, Ticket};

use crate::pages::{TicketActivitiesScreen, TicketBodyScreen};

use crate::context::TuiContext;
use crate::data::{ActionResult, DataPayload};
use crate::keymap::{Action, Keymap};
use crate::page::{Banner, BannerVariant, FooterCommand, StatusMessage};
use crate::screen::{Screen, ScreenResult};
use crate::widgets::{MiniProgressBar, ThemedTable};

/// Internal data-lifecycle state for ticket detail.
#[derive(Debug, Clone)]
enum DataState {
    /// Waiting for detail data.
    Loading,
    /// Data fetched successfully. `detail` is boxed to reduce enum size.
    Loaded {
        detail: Box<GetTicketResponse>,
        children: Vec<Ticket>,
        total_children: i32,
    },
    /// Fetch failed with this message.
    Error(String),
}

/// Screen showing the detail view for a single ticket.
///
/// Pushed onto the Tickets tab stack when `Action::Select` fires on a ticket.
/// Layout:
///   1. Header line (1 row): ID, title (truncated), status, progress bar.
///   2. Body preview (5 rows): first 4 lines of markdown-styled body, "..." if truncated.
///   3. Child table (min 3 rows): ThemedTable with same columns as the tickets list.
pub struct TicketDetailScreen {
    /// ID of the ticket being shown.
    ticket_id: String,
    /// Project the ticket belongs to (for create-child pre-fill).
    project: String,
    data_state: DataState,
    /// Selected row in the child table.
    selected_row: usize,
    /// Current page within the child table (server-side pagination).
    current_page: usize,
    page_size: usize,
    active_banner: Option<Banner>,
    active_status: Option<StatusMessage>,
}

impl TicketDetailScreen {
    /// Create a new detail screen for the given ticket ID and project.
    pub fn new(ticket_id: String, project: String) -> Self {
        Self {
            ticket_id,
            project,
            data_state: DataState::Loading,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
            active_banner: None,
            active_status: None,
        }
    }

    /// Returns the ticket ID this screen is displaying.
    pub fn ticket_id(&self) -> &str {
        &self.ticket_id
    }

    /// Returns the project key for this ticket.
    pub fn project(&self) -> &str {
        &self.project
    }

    /// Returns the pagination offset for child fetching.
    pub fn child_offset(&self) -> i32 {
        (self.current_page * self.page_size) as i32
    }

    /// Returns the page size for child fetching.
    pub fn child_page_size(&self) -> i32 {
        self.page_size as i32
    }

    /// Update page size based on available area, reserving header (1) + preview (5).
    pub fn update_child_page_size(&mut self, area_height: u16) {
        let chrome = 3u16; // table header + borders
        let reserved = 6u16; // header line + body preview
        let available = area_height.saturating_sub(reserved + chrome) as usize;
        if available > 0 && available != self.page_size {
            self.page_size = available;
            self.current_page = 0;
            self.selected_row = 0;
            self.mark_stale();
        }
    }

    /// Handle action result for banners.
    pub fn on_action_result(&mut self, result: &ActionResult) {
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

    fn total_pages(&self) -> usize {
        let total = match &self.data_state {
            DataState::Loaded { total_children, .. } => *total_children as usize,
            _ => 0,
        };
        if total == 0 || self.page_size == 0 {
            return 1;
        }
        total.div_ceil(self.page_size)
    }

    fn visible_children(&self) -> &[Ticket] {
        match &self.data_state {
            DataState::Loaded { children, .. } => children,
            _ => &[],
        }
    }

    fn selected_child(&self) -> Option<&Ticket> {
        self.visible_children().get(self.selected_row)
    }

    /// Returns the ticket ID of the currently selected child row.
    pub fn selected_child_id(&self) -> Option<String> {
        self.selected_child().map(|t| t.id.clone())
    }

    fn navigate_up(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
        }
    }

    fn navigate_down(&mut self) {
        let count = self.visible_children().len();
        if count > 0 && self.selected_row < count - 1 {
            self.selected_row += 1;
        }
    }

    fn page_left(&mut self) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.selected_row = 0;
            self.mark_stale();
        }
    }

    fn page_right(&mut self) {
        if self.current_page + 1 < self.total_pages() {
            self.current_page += 1;
            self.selected_row = 0;
            self.mark_stale();
        }
    }

    fn clamp_selection(&mut self) {
        let count = self.visible_children().len();
        if count == 0 {
            self.selected_row = 0;
        } else if self.selected_row >= count {
            self.selected_row = count.saturating_sub(1);
        }
    }

    fn dispatch_label(ticket: &Ticket) -> String {
        if !ticket.dispatch_status.is_empty() {
            "Dispatched".to_string()
        } else if ticket.status == "closed" {
            "Closed".to_string()
        } else {
            "Open".to_string()
        }
    }

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

    /// Push a `TicketBodyScreen` if the ticket data is loaded, otherwise consume.
    fn push_body_screen(&self) -> ScreenResult {
        let DataState::Loaded { detail, .. } = &self.data_state else {
            return ScreenResult::Consumed;
        };
        let Some(ticket) = &detail.ticket else {
            return ScreenResult::Consumed;
        };
        let body_screen =
            TicketBodyScreen::new(ticket.id.clone(), ticket.title.clone(), ticket.body.clone());
        ScreenResult::Push(Box::new(body_screen))
    }

    /// Push a `TicketActivitiesScreen` if the ticket data is loaded, otherwise consume.
    fn push_activities_screen(&self) -> ScreenResult {
        let DataState::Loaded { detail, .. } = &self.data_state else {
            return ScreenResult::Consumed;
        };
        let Some(ticket) = &detail.ticket else {
            return ScreenResult::Consumed;
        };
        let activities_screen =
            TicketActivitiesScreen::new(ticket.id.clone(), ticket.title.clone());
        ScreenResult::Push(Box::new(activities_screen))
    }

    fn build_child_rows(&self) -> Vec<Vec<String>> {
        self.visible_children()
            .iter()
            .map(|t| {
                vec![
                    t.id.clone(),
                    Self::dispatch_label(t),
                    t.priority.to_string(),
                    String::new(), // progress count placeholder
                    String::new(), // progress bar placeholder
                    t.title.clone(),
                ]
            })
            .collect()
    }

    fn render_message(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
        let style = Style::default()
            .fg(ctx.theme.base_content)
            .bg(ctx.theme.base_100);
        Paragraph::new(Line::raw(msg))
            .style(style)
            .render(area, buf);
    }
}

/// Column index of the progress count label in the child table.
const CHILD_PROGRESS_COUNT_COL: usize = 3;
/// Column index of the progress bar in the child table.
const CHILD_PROGRESS_BAR_COL: usize = 4;

/// Render mini progress bars over the child table placeholders.
fn render_child_progress_bars(
    screen: &TicketDetailScreen,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    widths: &[Constraint],
) {
    use ratatui::layout::Layout;

    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let col_areas = Layout::horizontal(widths.to_vec()).split(inner);
    let bar_area = col_areas.get(CHILD_PROGRESS_BAR_COL);
    let count_area = col_areas.get(CHILD_PROGRESS_COUNT_COL);

    if bar_area.is_none() && count_area.is_none() {
        return;
    }

    let data_start_y = inner.y + 1;
    let children = screen.visible_children();

    for (i, ticket) in children.iter().enumerate() {
        let row_y = data_start_y + i as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let (completed, total) = TicketDetailScreen::ticket_progress(ticket);
        let is_selected = i == screen.selected_row;
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

/// Build `MarkdownColors` from the TUI theme.
fn markdown_colors(ctx: &TuiContext) -> MarkdownColors {
    MarkdownColors {
        text: ctx.theme.base_content,
        heading: ctx.theme.accent,
        code: ctx.theme.warning,
        dim: ctx.theme.neutral_content,
    }
}

/// Render the header line: ID, title (truncated), status label, progress bar.
fn render_ticket_header(ticket: &Ticket, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
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
    let title_truncated = if ticket.title.chars().count() > title_budget {
        let s: String = ticket
            .title
            .chars()
            .take(title_budget.saturating_sub(1))
            .collect();
        format!("{s}…")
    } else {
        ticket.title.clone()
    };

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

    // Render the progress bar on the right.
    let (completed, total) = {
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
    };

    let bar = MiniProgressBar { completed, total };
    let bar_area = Rect {
        x: area.x + header_text_width + 1,
        y: area.y,
        width: progress_width,
        height: 1,
    };
    bar.render_bar(bar_area, buf, &ctx.theme, ctx.theme.base_100);
}

/// Render the body preview: up to 4 rendered markdown lines, with "..." if truncated.
fn render_body_preview(body: &str, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    const MAX_LINES: usize = 4;

    let colors = markdown_colors(ctx);
    let all_lines = render_markdown(body, area.width as usize, &colors);
    let truncated = all_lines.len() > MAX_LINES;
    let preview_lines: Vec<Line<'static>> = all_lines.into_iter().take(MAX_LINES).collect();

    let mut display_lines = preview_lines;
    if truncated {
        display_lines.push(Line::from(Span::styled(
            "...",
            Style::default().fg(ctx.theme.neutral_content),
        )));
    }

    let bg_style = Style::default().bg(ctx.theme.base_200);
    let para = Paragraph::new(display_lines).style(bg_style);
    para.render(area, buf);
}

/// Render the child table with navigation.
fn render_child_table(screen: &TicketDetailScreen, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let total_children = match &screen.data_state {
        DataState::Loaded { total_children, .. } => *total_children,
        _ => 0,
    };

    let rows = screen.build_child_rows();
    let page_info = if total_children > 0 {
        Some(format!(
            " Page {}/{} ({} children) ",
            screen.current_page + 1,
            screen.total_pages(),
            total_children,
        ))
    } else {
        None
    };

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
        selected: Some(screen.selected_row),
        widths: widths.clone(),
        page_info,
    };
    table.render(area, buf, ctx);

    render_child_progress_bars(screen, area, buf, ctx, &widths);
}

impl Screen for TicketDetailScreen {
    fn handle_action(&mut self, action: Action) -> ScreenResult {
        match action {
            Action::Back => ScreenResult::Pop,
            Action::Quit => ScreenResult::Quit,
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
                    text: "Refreshing...".to_string(),
                    dismissable: true,
                });
                ScreenResult::Consumed
            }
            // d → push TicketBodyScreen with the loaded ticket's body.
            Action::OpenDescription => self.push_body_screen(),
            // a → push TicketActivitiesScreen.
            Action::OpenActivities => self.push_activities_screen(),
            // Space → drill down into selected child
            Action::Select => {
                if let Some(child) = self.selected_child() {
                    let child_id = child.id.clone();
                    let child_project = child.project.clone();
                    let child_screen = TicketDetailScreen::new(child_id, child_project);
                    ScreenResult::Push(Box::new(child_screen))
                } else {
                    ScreenResult::Consumed
                }
            }
            // Ticket commands on selected child (handled by app via dispatch; we just consume)
            Action::Dispatch
            | Action::DispatchAll
            | Action::LaunchDesign
            | Action::CloseTicket
            | Action::CancelFlow
            | Action::CreateTicket => ScreenResult::Consumed,
            Action::SetPriority => ScreenResult::Consumed,
            Action::Filter => ScreenResult::Consumed,
            _ => ScreenResult::Ignored,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let chunks = Layout::vertical([
            Constraint::Length(1), // header line
            Constraint::Length(5), // body preview (4 lines + 1 for "...")
            Constraint::Min(3),    // child table
        ])
        .split(area);

        match &self.data_state {
            DataState::Loading => {
                self.render_message(area, buf, ctx, "Loading...");
            }
            DataState::Error(msg) => {
                self.render_message(area, buf, ctx, &format!("Error: {msg}"));
            }
            DataState::Loaded { detail, .. } => {
                if let Some(ticket) = &detail.ticket {
                    render_ticket_header(ticket, chunks[0], buf, ctx);
                    render_body_preview(&ticket.body, chunks[1], buf, ctx);
                } else {
                    self.render_message(chunks[0], buf, ctx, &self.ticket_id);
                }
                render_child_table(self, chunks[2], buf, ctx);
            }
        }
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: keymap.label_for(&Action::DispatchAll),
                description: "Dispatch all".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::CreateTicket),
                description: "Create child".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Dispatch),
                description: "Dispatch".to_string(),
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
                key_label: keymap.label_for(&Action::OpenActivities),
                description: "Activities".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::OpenDescription),
                description: "Description".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Select),
                description: "Open child".to_string(),
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
                key_label: keymap.label_for(&Action::Quit),
                description: "Quit".to_string(),
                common: true,
            },
        ]
    }

    fn on_data(&mut self, payload: &DataPayload) {
        let DataPayload::TicketDetail(boxed) = payload else {
            return;
        };
        match boxed.as_ref() {
            Ok((detail, children, total)) => {
                debug!(
                    ticket_id = %self.ticket_id,
                    children_count = children.len(),
                    total_children = total,
                    "ticket_detail: Loading -> Loaded"
                );
                self.active_status = None;
                self.data_state = DataState::Loaded {
                    detail: Box::new(detail.clone()),
                    children: children.clone(),
                    total_children: *total,
                };
                self.clamp_selection();
            }
            Err(msg) => {
                debug!(
                    ticket_id = %self.ticket_id,
                    error = %msg,
                    "ticket_detail: Loading -> Error"
                );
                self.active_status = None;
                self.data_state = DataState::Error(msg.clone());
            }
        }
    }

    fn needs_data(&self) -> bool {
        matches!(self.data_state, DataState::Loading)
    }

    fn mark_stale(&mut self) {
        debug!(ticket_id = %self.ticket_id, "ticket_detail: mark_stale");
        self.data_state = DataState::Loading;
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

    fn as_any_ticket_detail(&self) -> Option<&crate::pages::TicketDetailScreen> {
        Some(self)
    }

    fn as_any_ticket_detail_mut(&mut self) -> Option<&mut crate::pages::TicketDetailScreen> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ur_rpc::proto::ticket::GetTicketResponse;

    fn make_ticket(id: &str, project: &str, status: &str) -> Ticket {
        Ticket {
            id: id.to_string(),
            ticket_type: "task".to_string(),
            status: status.to_string(),
            priority: 2,
            parent_id: String::new(),
            title: format!("Ticket {id}"),
            body: String::new(),
            created_at: String::new(),
            updated_at: String::new(),
            project: project.to_string(),
            branch: String::new(),
            depth: 0,
            children_total: 0,
            children_completed: 0,
            dispatch_status: String::new(),
        }
    }

    fn make_detail_response(ticket: Ticket) -> GetTicketResponse {
        GetTicketResponse {
            ticket: Some(ticket),
            metadata: vec![],
            activities: vec![],
        }
    }

    #[test]
    fn new_screen_needs_data() {
        let screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        assert!(screen.needs_data());
        assert_eq!(screen.ticket_id(), "ur-abc");
        assert_eq!(screen.project(), "ur");
    }

    #[test]
    fn on_data_ticket_detail_ok() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        let ticket = make_ticket("ur-abc", "ur", "open");
        let detail = make_detail_response(ticket.clone());
        let children = vec![make_ticket("ur-child1", "ur", "open")];
        screen.on_data(&DataPayload::TicketDetail(Box::new(Ok((
            detail, children, 1,
        )))));
        assert!(!screen.needs_data());
        assert_eq!(screen.visible_children().len(), 1);
    }

    #[test]
    fn on_data_ticket_detail_error() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        screen.on_data(&DataPayload::TicketDetail(Box::new(Err(
            "rpc failed".into()
        ))));
        assert!(!screen.needs_data());
        assert!(matches!(screen.data_state, DataState::Error(_)));
    }

    #[test]
    fn on_data_ignores_other_payloads() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        screen.on_data(&DataPayload::Tickets(Ok((vec![], 0))));
        assert!(screen.needs_data()); // still loading
    }

    #[test]
    fn back_action_returns_pop() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        assert!(matches!(
            screen.handle_action(Action::Back),
            ScreenResult::Pop
        ));
    }

    #[test]
    fn quit_action_returns_quit() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        assert!(matches!(
            screen.handle_action(Action::Quit),
            ScreenResult::Quit
        ));
    }

    #[test]
    fn navigate_up_down() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        let ticket = make_ticket("ur-abc", "ur", "open");
        let detail = make_detail_response(ticket);
        let children: Vec<Ticket> = (0..3)
            .map(|i| make_ticket(&format!("ur-c{i}"), "ur", "open"))
            .collect();
        screen.on_data(&DataPayload::TicketDetail(Box::new(Ok((
            detail, children, 3,
        )))));

        assert_eq!(screen.selected_row, 0);
        screen.handle_action(Action::NavigateDown);
        assert_eq!(screen.selected_row, 1);
        screen.handle_action(Action::NavigateUp);
        assert_eq!(screen.selected_row, 0);
        screen.handle_action(Action::NavigateUp);
        assert_eq!(screen.selected_row, 0); // no underflow
    }

    #[test]
    fn navigate_down_does_not_overflow() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        let ticket = make_ticket("ur-abc", "ur", "open");
        let detail = make_detail_response(ticket);
        let children = vec![
            make_ticket("ur-c0", "ur", "open"),
            make_ticket("ur-c1", "ur", "open"),
        ];
        screen.on_data(&DataPayload::TicketDetail(Box::new(Ok((
            detail, children, 2,
        )))));

        screen.handle_action(Action::NavigateDown);
        screen.handle_action(Action::NavigateDown);
        screen.handle_action(Action::NavigateDown);
        assert_eq!(screen.selected_row, 1); // clamped at last
    }

    #[test]
    fn select_with_child_pushes_detail_screen() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        let ticket = make_ticket("ur-abc", "ur", "open");
        let detail = make_detail_response(ticket);
        let children = vec![make_ticket("ur-child1", "ur", "open")];
        screen.on_data(&DataPayload::TicketDetail(Box::new(Ok((
            detail, children, 1,
        )))));

        let result = screen.handle_action(Action::Select);
        assert!(matches!(result, ScreenResult::Push(_)));
    }

    #[test]
    fn select_with_no_children_consumed() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        let ticket = make_ticket("ur-abc", "ur", "open");
        let detail = make_detail_response(ticket);
        screen.on_data(&DataPayload::TicketDetail(Box::new(Ok((
            detail,
            vec![],
            0,
        )))));

        let result = screen.handle_action(Action::Select);
        assert!(matches!(result, ScreenResult::Consumed));
    }

    #[test]
    fn refresh_marks_stale() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        let ticket = make_ticket("ur-abc", "ur", "open");
        let detail = make_detail_response(ticket);
        screen.on_data(&DataPayload::TicketDetail(Box::new(Ok((
            detail,
            vec![],
            0,
        )))));
        assert!(!screen.needs_data());

        screen.handle_action(Action::Refresh);
        assert!(screen.needs_data());
    }

    #[test]
    fn footer_commands_not_empty() {
        let screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        let keymap = Keymap::default();
        let cmds = screen.footer_commands(&keymap);
        assert!(!cmds.is_empty());
        // Must contain Back
        assert!(cmds.iter().any(|c| c.description == "Back"));
    }

    #[test]
    fn selected_child_id_returns_correct_id() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        let ticket = make_ticket("ur-abc", "ur", "open");
        let detail = make_detail_response(ticket);
        let children = vec![
            make_ticket("ur-c0", "ur", "open"),
            make_ticket("ur-c1", "ur", "open"),
        ];
        screen.on_data(&DataPayload::TicketDetail(Box::new(Ok((
            detail, children, 2,
        )))));

        assert_eq!(screen.selected_child_id(), Some("ur-c0".to_string()));
        screen.handle_action(Action::NavigateDown);
        assert_eq!(screen.selected_child_id(), Some("ur-c1".to_string()));
    }

    #[test]
    fn pagination_page_right_and_left() {
        let mut screen = TicketDetailScreen::new("ur-abc".to_string(), "ur".to_string());
        screen.page_size = 2;
        let ticket = make_ticket("ur-abc", "ur", "open");
        let detail = make_detail_response(ticket);
        let children = vec![
            make_ticket("ur-c0", "ur", "open"),
            make_ticket("ur-c1", "ur", "open"),
        ];
        screen.on_data(&DataPayload::TicketDetail(Box::new(Ok((
            detail, children, 5,
        )))));

        assert_eq!(screen.current_page, 0);
        screen.handle_action(Action::PageRight);
        assert_eq!(screen.current_page, 1);
        assert!(screen.needs_data());

        // Can't go before first page
        screen.handle_action(Action::PageLeft);
        assert_eq!(screen.current_page, 0);
    }

    #[test]
    fn dispatch_label_reflects_status() {
        let mut t = make_ticket("ur-x", "ur", "open");
        assert_eq!(TicketDetailScreen::dispatch_label(&t), "Open");
        t.status = "closed".to_string();
        assert_eq!(TicketDetailScreen::dispatch_label(&t), "Closed");
        t.dispatch_status = "implementing".to_string();
        assert_eq!(TicketDetailScreen::dispatch_label(&t), "Dispatched");
    }
}
