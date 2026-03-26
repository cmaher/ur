use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use ur_rpc::proto::ticket::ActivityEntry;

use crate::context::TuiContext;
use crate::data::DataPayload;
use crate::keymap::{Action, Keymap};
use crate::page::FooterCommand;
use crate::screen::{Screen, ScreenResult};
use crate::widgets::ThemedTable;

/// Internal data-lifecycle state for ticket activities.
#[derive(Debug, Clone)]
enum DataState {
    /// Waiting for activities data.
    Loading,
    /// Activities fetched successfully.
    Loaded(Vec<ActivityEntry>),
    /// Fetch failed with this message.
    Error(String),
}

/// Full-screen activities viewer for a single ticket.
///
/// Pushed from `TicketDetailScreen` when `Action::OpenActivities` fires.
/// Issues its own `GetTicket` call via `DataPayload::TicketActivities` so it
/// can independently apply an author filter and refresh without affecting the
/// detail screen.
///
/// Layout:
///   1. Header (Length(1)): ticket ID, title (truncated), activity count.
///   2. Filter bar (Length(1), only when a filter is active): current author filter.
///   3. Table (Min(3)): ThemedTable with Timestamp / Author / Message columns.
pub struct TicketActivitiesScreen {
    ticket_id: String,
    ticket_title: String,
    data_state: DataState,
    /// Selected row within the current page.
    selected_row: usize,
    /// Current page (client-side pagination).
    current_page: usize,
    page_size: usize,
    /// Unique authors extracted from the last successful fetch.
    /// Index 0 is always "all" (no filter). Indices 1..N are specific authors.
    authors: Vec<String>,
    /// Currently selected author index (0 = all).
    author_index: usize,
}

impl TicketActivitiesScreen {
    /// Create a new activities screen for the given ticket.
    ///
    /// `ticket_id` — used to fetch activities and shown in the header.
    /// `ticket_title` — shown in the header alongside the ID.
    pub fn new(ticket_id: String, ticket_title: String) -> Self {
        Self {
            ticket_id,
            ticket_title,
            data_state: DataState::Loading,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
            authors: vec!["all".to_string()],
            author_index: 0,
        }
    }

    /// Returns the ticket ID this screen is displaying.
    pub fn ticket_id(&self) -> &str {
        &self.ticket_id
    }

    /// Returns the currently active author filter, or `None` for "all".
    pub fn author_filter(&self) -> Option<&str> {
        if self.author_index == 0 {
            None
        } else {
            self.authors.get(self.author_index).map(String::as_str)
        }
    }

    /// Returns whether a non-"all" author filter is currently active.
    fn has_active_filter(&self) -> bool {
        self.author_index != 0
    }

    /// All activities from the current data state (unfiltered by client-side logic —
    /// filtering is server-side via `author_filter`).
    fn all_activities(&self) -> &[ActivityEntry] {
        match &self.data_state {
            DataState::Loaded(entries) => entries,
            _ => &[],
        }
    }

    /// Activities reversed to newest-first order.
    fn reversed_activities(&self) -> Vec<&ActivityEntry> {
        self.all_activities().iter().rev().collect()
    }

    fn total_activities(&self) -> usize {
        self.all_activities().len()
    }

    fn total_pages(&self) -> usize {
        let total = self.total_activities();
        if total == 0 || self.page_size == 0 {
            return 1;
        }
        total.div_ceil(self.page_size)
    }

    /// Activities on the current page (newest-first slice).
    fn page_activities(&self) -> Vec<&ActivityEntry> {
        let all = self.reversed_activities();
        let start = self.current_page * self.page_size;
        let end = (start + self.page_size).min(all.len());
        all[start..end].to_vec()
    }

    fn navigate_up(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
        }
    }

    fn navigate_down(&mut self) {
        let count = self.page_activities().len();
        if count > 0 && self.selected_row < count - 1 {
            self.selected_row += 1;
        }
    }

    fn page_left(&mut self) {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.selected_row = 0;
        }
    }

    fn page_right(&mut self) {
        if self.current_page + 1 < self.total_pages() {
            self.current_page += 1;
            self.selected_row = 0;
        }
    }

    fn clamp_selection(&mut self) {
        let count = self.page_activities().len();
        if count == 0 {
            self.selected_row = 0;
        } else if self.selected_row >= count {
            self.selected_row = count.saturating_sub(1);
        }
    }

    /// Cycle to the next author in the author list and mark stale so the data
    /// manager re-fetches with the new filter applied server-side.
    fn cycle_author_filter(&mut self) {
        if self.authors.len() > 1 {
            self.author_index = (self.author_index + 1) % self.authors.len();
            self.current_page = 0;
            self.selected_row = 0;
            self.mark_stale();
        }
    }

    /// Rebuild the unique author list from the given activities.
    ///
    /// Index 0 is always "all". Authors appear in insertion order (i.e., the
    /// order they first appear in the activities list, oldest-first as returned
    /// by the server).
    fn rebuild_authors(activities: &[ActivityEntry]) -> Vec<String> {
        let mut seen = Vec::new();
        seen.push("all".to_string());
        for entry in activities {
            if !entry.author.is_empty() && !seen.contains(&entry.author) {
                seen.push(entry.author.clone());
            }
        }
        seen
    }

    fn render_message(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
        let style = Style::default()
            .fg(ctx.theme.base_content)
            .bg(ctx.theme.base_100);
        Paragraph::new(Line::raw(msg))
            .style(style)
            .render(area, buf);
    }

    fn build_table_rows(&self) -> Vec<Vec<String>> {
        self.page_activities()
            .iter()
            .map(|entry| {
                vec![
                    format_timestamp(&entry.timestamp),
                    truncate(&entry.author, 18),
                    entry.message.lines().next().unwrap_or("").to_string(),
                ]
            })
            .collect()
    }
}

/// Format a timestamp string to `YYYY-MM-DD HH:MM:SS` (20 chars), truncating
/// or padding as needed.
fn format_timestamp(ts: &str) -> String {
    // Timestamps from the server are ISO-8601; keep the first 19 chars (date + time)
    // and replace 'T' with a space for readability.
    let s = ts.replace('T', " ");
    let trimmed = s.trim_end_matches('Z').trim_end_matches(|c: char| {
        // Drop sub-second precision and timezone offsets beyond the seconds field.
        !c.is_ascii_digit() && c != ' ' && c != '-' && c != ':'
    });
    let candidate: String = trimmed.chars().take(19).collect();
    if candidate.len() == 19 {
        candidate
    } else {
        // Pad or return as-is if format is unexpected.
        format!("{:<19}", candidate)
    }
}

/// Truncate a string to at most `max_chars` Unicode characters.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{truncated}…")
    }
}

/// Render the header line: ticket ID (accent) + title + activity count.
fn render_activities_header(
    ticket_id: &str,
    title: &str,
    activity_count: usize,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
) {
    let id_style = Style::default().fg(ctx.theme.accent);
    let title_style = Style::default().fg(ctx.theme.base_content);
    let count_style = Style::default().fg(ctx.theme.neutral_content);

    let id_part = format!("{ticket_id}  ");
    let count_part = format!("  Activities ({activity_count})");

    let title_budget = (area.width as usize)
        .saturating_sub(id_part.len() + count_part.len())
        .max(1);

    let title_truncated = truncate(title, title_budget);

    let line = Line::from(vec![
        Span::styled(id_part, id_style),
        Span::styled(title_truncated, title_style),
        Span::styled(count_part, count_style),
    ]);

    Paragraph::new(line).render(area, buf);
}

/// Render the filter bar (shown only when a non-"all" filter is active).
fn render_filter_bar(author: &str, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let label_style = Style::default().fg(ctx.theme.neutral_content);
    let value_style = Style::default().fg(ctx.theme.accent);

    let line = Line::from(vec![
        Span::styled("Filter: [", label_style),
        Span::styled(author.to_string(), value_style),
        Span::styled("]", label_style),
    ]);

    let bg_style = Style::default().bg(ctx.theme.base_200);
    Paragraph::new(line).style(bg_style).render(area, buf);
}

/// Render the activities table.
fn render_activities_table(
    screen: &TicketActivitiesScreen,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
) {
    let total = screen.total_activities();
    let page_info = if total > 0 {
        Some(format!(
            " Page {}/{} ({} activities) ",
            screen.current_page + 1,
            screen.total_pages(),
            total,
        ))
    } else {
        None
    };

    let rows = screen.build_table_rows();
    let widths = vec![
        Constraint::Length(20),
        Constraint::Length(18),
        Constraint::Fill(1),
    ];

    let table = ThemedTable {
        headers: vec!["Timestamp", "Author", "Message"],
        rows,
        selected: Some(screen.selected_row),
        widths,
        page_info,
    };

    table.render(area, buf, ctx);
}

impl Screen for TicketActivitiesScreen {
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
                ScreenResult::Consumed
            }
            Action::Filter => {
                self.cycle_author_filter();
                ScreenResult::Consumed
            }
            _ => ScreenResult::Ignored,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        let show_filter = self.has_active_filter();
        let filter_rows = if show_filter { 1 } else { 0 };

        match &self.data_state {
            DataState::Loading => {
                self.render_message(area, buf, ctx, "Loading...");
            }
            DataState::Error(msg) => {
                self.render_message(area, buf, ctx, &format!("Error: {msg}"));
            }
            DataState::Loaded(_) => {
                let chunks = if show_filter {
                    Layout::vertical([
                        Constraint::Length(1),           // header
                        Constraint::Length(filter_rows), // filter bar
                        Constraint::Min(3),              // table
                    ])
                    .split(area)
                } else {
                    Layout::vertical([
                        Constraint::Length(1), // header
                        Constraint::Min(3),    // table
                    ])
                    .split(area)
                };

                let activity_count = self.total_activities();
                render_activities_header(
                    &self.ticket_id,
                    &self.ticket_title,
                    activity_count,
                    chunks[0],
                    buf,
                    ctx,
                );

                if show_filter {
                    let author = self
                        .authors
                        .get(self.author_index)
                        .map(String::as_str)
                        .unwrap_or("unknown");
                    render_filter_bar(author, chunks[1], buf, ctx);
                    render_activities_table(self, chunks[2], buf, ctx);
                } else {
                    render_activities_table(self, chunks[1], buf, ctx);
                }
            }
        }
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: keymap.combined_label(&Action::NavigateUp, &Action::NavigateDown),
                description: "Scroll".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.combined_label(&Action::PageLeft, &Action::PageRight),
                description: "Page".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::Filter),
                description: "Filter author".to_string(),
                common: false,
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
        if let DataPayload::TicketActivities(result) = payload {
            match result {
                Ok(activities) => {
                    // Rebuild unique authors from the full (unfiltered) list only
                    // when we switch to "all" (author_index == 0), so that cycling
                    // is consistent across filtered fetches.
                    if self.author_index == 0 {
                        self.authors = Self::rebuild_authors(activities);
                    }
                    self.data_state = DataState::Loaded(activities.clone());
                    self.clamp_selection();
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

    fn mark_stale(&mut self) {
        self.data_state = DataState::Loading;
    }

    fn as_any_ticket_activities(&self) -> Option<&TicketActivitiesScreen> {
        Some(self)
    }

    fn as_any_ticket_activities_mut(&mut self) -> Option<&mut TicketActivitiesScreen> {
        Some(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ur_rpc::proto::ticket::ActivityEntry;

    fn make_entry(author: &str, timestamp: &str, message: &str) -> ActivityEntry {
        ActivityEntry {
            id: format!("act-{author}"),
            timestamp: timestamp.to_string(),
            author: author.to_string(),
            message: message.to_string(),
        }
    }

    fn make_screen() -> TicketActivitiesScreen {
        TicketActivitiesScreen::new("ur-test".to_string(), "Test Ticket".to_string())
    }

    fn load_screen(screen: &mut TicketActivitiesScreen, entries: Vec<ActivityEntry>) {
        screen.on_data(&DataPayload::TicketActivities(Ok(entries)));
    }

    // ── construction ──────────────────────────────────────────────────────

    #[test]
    fn new_screen_needs_data() {
        let screen = make_screen();
        assert!(screen.needs_data());
        assert_eq!(screen.ticket_id(), "ur-test");
    }

    #[test]
    fn new_screen_has_no_filter() {
        let screen = make_screen();
        assert_eq!(screen.author_filter(), None);
        assert!(!screen.has_active_filter());
    }

    // ── on_data ───────────────────────────────────────────────────────────

    #[test]
    fn on_data_ok_marks_loaded() {
        let mut screen = make_screen();
        let entries = vec![make_entry("alice", "2026-03-25T14:32:10Z", "hello")];
        load_screen(&mut screen, entries);
        assert!(!screen.needs_data());
    }

    #[test]
    fn on_data_error_marks_error() {
        let mut screen = make_screen();
        screen.on_data(&DataPayload::TicketActivities(Err("rpc failed".into())));
        assert!(!screen.needs_data());
        assert!(matches!(screen.data_state, DataState::Error(_)));
    }

    #[test]
    fn on_data_ignores_other_payloads() {
        let mut screen = make_screen();
        screen.on_data(&DataPayload::Tickets(Ok((vec![], 0))));
        assert!(screen.needs_data()); // still loading
    }

    #[test]
    fn on_data_rebuilds_authors_when_unfiltered() {
        let mut screen = make_screen();
        let entries = vec![
            make_entry("alice", "2026-01-01T00:00:00Z", "msg1"),
            make_entry("bob", "2026-01-02T00:00:00Z", "msg2"),
            make_entry("alice", "2026-01-03T00:00:00Z", "msg3"),
        ];
        load_screen(&mut screen, entries);
        // "all" + "alice" + "bob"
        assert_eq!(screen.authors.len(), 3);
        assert_eq!(screen.authors[0], "all");
        assert_eq!(screen.authors[1], "alice");
        assert_eq!(screen.authors[2], "bob");
    }

    // ── newest-first ordering ─────────────────────────────────────────────

    #[test]
    fn activities_are_reversed_newest_first() {
        let mut screen = make_screen();
        let entries = vec![
            make_entry("alice", "2026-01-01T00:00:00Z", "oldest"),
            make_entry("bob", "2026-01-02T00:00:00Z", "middle"),
            make_entry("carol", "2026-01-03T00:00:00Z", "newest"),
        ];
        load_screen(&mut screen, entries);
        let page = screen.page_activities();
        assert_eq!(page[0].message, "newest");
        assert_eq!(page[2].message, "oldest");
    }

    // ── pagination ────────────────────────────────────────────────────────

    #[test]
    fn total_pages_with_no_activities_is_one() {
        let mut screen = make_screen();
        load_screen(&mut screen, vec![]);
        assert_eq!(screen.total_pages(), 1);
    }

    #[test]
    fn total_pages_rounds_up() {
        let mut screen = make_screen();
        screen.page_size = 2;
        let entries: Vec<_> = (0..5)
            .map(|i| make_entry("a", "2026-01-01T00:00:00Z", &format!("msg{i}")))
            .collect();
        load_screen(&mut screen, entries);
        // 5 activities / 2 per page = 3 pages
        assert_eq!(screen.total_pages(), 3);
    }

    #[test]
    fn page_activities_returns_correct_slice() {
        let mut screen = make_screen();
        screen.page_size = 2;
        // 4 entries: newest-first after reverse will be [3,2,1,0]
        let entries: Vec<_> = (0..4)
            .map(|i| make_entry("a", "2026-01-01T00:00:00Z", &format!("msg{i}")))
            .collect();
        load_screen(&mut screen, entries);
        // page 0 → first 2 of reversed = msg3, msg2
        assert_eq!(screen.page_activities().len(), 2);
        screen.page_right();
        // page 1 → msg1, msg0
        assert_eq!(screen.page_activities().len(), 2);
    }

    #[test]
    fn page_right_stops_at_last_page() {
        let mut screen = make_screen();
        screen.page_size = 2;
        let entries: Vec<_> = (0..3)
            .map(|i| make_entry("a", "2026-01-01T00:00:00Z", &format!("msg{i}")))
            .collect();
        load_screen(&mut screen, entries);
        // 2 pages (3 entries, page_size 2)
        screen.page_right();
        assert_eq!(screen.current_page, 1);
        screen.page_right(); // already on last
        assert_eq!(screen.current_page, 1);
    }

    #[test]
    fn page_left_stops_at_first_page() {
        let mut screen = make_screen();
        screen.page_left();
        assert_eq!(screen.current_page, 0);
    }

    // ── navigation ────────────────────────────────────────────────────────

    #[test]
    fn navigate_up_clamps_to_zero() {
        let mut screen = make_screen();
        let entries = vec![make_entry("a", "2026-01-01T00:00:00Z", "msg")];
        load_screen(&mut screen, entries);
        screen.navigate_up();
        assert_eq!(screen.selected_row, 0);
    }

    #[test]
    fn navigate_down_does_not_overflow() {
        let mut screen = make_screen();
        let entries = vec![
            make_entry("a", "2026-01-01T00:00:00Z", "msg0"),
            make_entry("b", "2026-01-02T00:00:00Z", "msg1"),
        ];
        load_screen(&mut screen, entries);
        screen.navigate_down();
        assert_eq!(screen.selected_row, 1);
        screen.navigate_down();
        assert_eq!(screen.selected_row, 1); // clamped
    }

    // ── author filter cycling ─────────────────────────────────────────────

    #[test]
    fn filter_cycling_advances_author_index() {
        let mut screen = make_screen();
        let entries = vec![
            make_entry("alice", "2026-01-01T00:00:00Z", "msg1"),
            make_entry("bob", "2026-01-02T00:00:00Z", "msg2"),
        ];
        load_screen(&mut screen, entries);
        // authors: [all, alice, bob]
        assert_eq!(screen.author_filter(), None); // index 0 = all
        screen.cycle_author_filter();
        assert_eq!(screen.author_filter(), Some("alice"));
        screen.cycle_author_filter();
        assert_eq!(screen.author_filter(), Some("bob"));
        screen.cycle_author_filter();
        assert_eq!(screen.author_filter(), None); // wrapped back to all
    }

    #[test]
    fn filter_cycling_with_no_authors_is_noop() {
        let mut screen = make_screen();
        load_screen(&mut screen, vec![]);
        // Only "all" in authors; cycling should not panic or change anything.
        screen.cycle_author_filter();
        assert_eq!(screen.author_filter(), None);
    }

    #[test]
    fn filter_cycling_marks_stale() {
        let mut screen = make_screen();
        let entries = vec![
            make_entry("alice", "2026-01-01T00:00:00Z", "msg"),
            make_entry("bob", "2026-01-02T00:00:00Z", "msg2"),
        ];
        load_screen(&mut screen, entries);
        assert!(!screen.needs_data());
        screen.cycle_author_filter();
        assert!(screen.needs_data());
    }

    #[test]
    fn author_filter_returns_none_for_all() {
        let mut screen = make_screen();
        assert_eq!(screen.author_filter(), None);
    }

    // ── action handling ───────────────────────────────────────────────────

    #[test]
    fn back_action_returns_pop() {
        let mut screen = make_screen();
        assert!(matches!(
            screen.handle_action(Action::Back),
            ScreenResult::Pop
        ));
    }

    #[test]
    fn quit_action_returns_quit() {
        let mut screen = make_screen();
        assert!(matches!(
            screen.handle_action(Action::Quit),
            ScreenResult::Quit
        ));
    }

    #[test]
    fn filter_action_cycles_author() {
        let mut screen = make_screen();
        let entries = vec![
            make_entry("alice", "2026-01-01T00:00:00Z", "msg"),
            make_entry("bob", "2026-01-02T00:00:00Z", "msg2"),
        ];
        load_screen(&mut screen, entries);
        assert_eq!(screen.author_filter(), None);
        screen.handle_action(Action::Filter);
        assert_eq!(screen.author_filter(), Some("alice"));
    }

    #[test]
    fn refresh_action_marks_stale() {
        let mut screen = make_screen();
        let entries = vec![make_entry("alice", "2026-01-01T00:00:00Z", "msg")];
        load_screen(&mut screen, entries);
        assert!(!screen.needs_data());
        screen.handle_action(Action::Refresh);
        assert!(screen.needs_data());
    }

    #[test]
    fn unhandled_action_returns_ignored() {
        let mut screen = make_screen();
        assert!(matches!(
            screen.handle_action(Action::Dispatch),
            ScreenResult::Ignored
        ));
    }

    #[test]
    fn navigate_up_and_down_returns_consumed() {
        let mut screen = make_screen();
        let entries = vec![
            make_entry("a", "2026-01-01T00:00:00Z", "m1"),
            make_entry("b", "2026-01-02T00:00:00Z", "m2"),
        ];
        load_screen(&mut screen, entries);
        assert!(matches!(
            screen.handle_action(Action::NavigateDown),
            ScreenResult::Consumed
        ));
        assert!(matches!(
            screen.handle_action(Action::NavigateUp),
            ScreenResult::Consumed
        ));
    }

    // ── downcast ──────────────────────────────────────────────────────────

    #[test]
    fn downcast_returns_self() {
        let screen = make_screen();
        assert!(screen.as_any_ticket_activities().is_some());
    }

    #[test]
    fn downcast_mut_returns_self() {
        let mut screen = make_screen();
        assert!(screen.as_any_ticket_activities_mut().is_some()); // requires &mut self
    }

    // ── mark_stale / needs_data ───────────────────────────────────────────

    #[test]
    fn mark_stale_resets_to_loading() {
        let mut screen = make_screen();
        let entries = vec![make_entry("a", "2026-01-01T00:00:00Z", "msg")];
        load_screen(&mut screen, entries);
        assert!(!screen.needs_data());
        screen.mark_stale();
        assert!(screen.needs_data());
    }

    // ── footer_commands ───────────────────────────────────────────────────

    #[test]
    fn footer_has_back_and_quit() {
        let screen = make_screen();
        let keymap = Keymap::default();
        let cmds = screen.footer_commands(&keymap);
        assert!(cmds.iter().any(|c| c.description == "Back"));
        assert!(cmds.iter().any(|c| c.description == "Quit"));
    }

    #[test]
    fn footer_has_filter_author() {
        let screen = make_screen();
        let keymap = Keymap::default();
        let cmds = screen.footer_commands(&keymap);
        assert!(cmds.iter().any(|c| c.description == "Filter author"));
    }

    // ── format_timestamp ─────────────────────────────────────────────────

    #[test]
    fn format_timestamp_iso8601_with_z() {
        let ts = format_timestamp("2026-03-25T14:32:10Z");
        assert_eq!(ts, "2026-03-25 14:32:10");
    }

    #[test]
    fn format_timestamp_iso8601_with_subseconds() {
        let ts = format_timestamp("2026-03-25T14:32:10.123456Z");
        // After replace T->space: "2026-03-25 14:32:10.123456Z", take 19 = "2026-03-25 14:32:10"
        assert_eq!(ts, "2026-03-25 14:32:10");
    }

    // ── rebuild_authors ───────────────────────────────────────────────────

    #[test]
    fn rebuild_authors_deduplicates_and_preserves_order() {
        let entries = vec![
            make_entry("alice", "", ""),
            make_entry("bob", "", ""),
            make_entry("alice", "", ""),
        ];
        let authors = TicketActivitiesScreen::rebuild_authors(&entries);
        assert_eq!(authors, vec!["all", "alice", "bob"]);
    }

    #[test]
    fn rebuild_authors_empty_returns_only_all() {
        let authors = TicketActivitiesScreen::rebuild_authors(&[]);
        assert_eq!(authors, vec!["all"]);
    }

    // ── truncate ─────────────────────────────────────────────────────────

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let s = truncate("hello world this is a long string", 10);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 10);
    }
}
