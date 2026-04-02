use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::cmd::{Cmd, FetchCmd};
use crate::components::ThemedTable;
use crate::context::TuiContext;
use crate::input::{FooterCommand, InputHandler, InputResult};
use crate::model::{LoadState, Model, TicketActivitiesData};
use crate::msg::{Msg, NavMsg};

/// Render the ticket activities page into the given content area.
///
/// Shows a header with ticket ID and title, an optional author filter bar,
/// and a table of activities with Timestamp / Author / Message columns.
/// Activities are shown newest-first with client-side pagination.
pub fn render_ticket_activities(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    let Some(ref activities_model) = model.ticket_activities else {
        render_message(area, buf, ctx, "No activities data");
        return;
    };

    match &activities_model.data {
        LoadState::NotLoaded | LoadState::Loading => {
            render_message(area, buf, ctx, "Loading...");
        }
        LoadState::Error(msg) => {
            render_message(area, buf, ctx, &format!("Error: {msg}"));
        }
        LoadState::Loaded(data) => {
            render_loaded_activities(area, buf, ctx, activities_model, data);
        }
    }
}

/// Render the activities page when data is loaded.
fn render_loaded_activities(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    activities_model: &super::super::model::TicketActivitiesModel,
    data: &TicketActivitiesData,
) {
    let show_filter = activities_model.author_index != 0;

    let chunks = if show_filter {
        Layout::vertical([
            Constraint::Length(1), // header
            Constraint::Length(1), // filter bar
            Constraint::Min(3),    // table
        ])
        .split(area)
    } else {
        Layout::vertical([
            Constraint::Length(1), // header
            Constraint::Min(3),    // table
        ])
        .split(area)
    };

    render_activities_header(
        &activities_model.ticket_id,
        &activities_model.ticket_title,
        data.activities.len(),
        chunks[0],
        buf,
        ctx,
    );

    if show_filter {
        let author = activities_model
            .authors
            .get(activities_model.author_index)
            .map(String::as_str)
            .unwrap_or("unknown");
        render_filter_bar(author, chunks[1], buf, ctx);
        render_activities_table(activities_model, data, chunks[2], buf, ctx);
    } else {
        render_activities_table(activities_model, data, chunks[1], buf, ctx);
    }
}

/// Render a simple centered message.
fn render_message(area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
    let style = Style::default()
        .fg(ctx.theme.base_content)
        .bg(ctx.theme.base_100);
    Paragraph::new(Line::raw(msg))
        .style(style)
        .render(area, buf);
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

/// Render the activities table with pagination.
fn render_activities_table(
    activities_model: &super::super::model::TicketActivitiesModel,
    data: &TicketActivitiesData,
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
) {
    let total = data.activities.len();
    let page_size = activities_model.page_size;
    let current_page = activities_model.current_page;

    let total_pages = if total == 0 || page_size == 0 {
        1
    } else {
        total.div_ceil(page_size)
    };

    let page_info = if total > 0 {
        Some(format!(
            " Page {}/{} ({} activities) ",
            current_page + 1,
            total_pages,
            total,
        ))
    } else {
        None
    };

    let rows = build_table_rows(data, current_page, page_size);
    let widths = vec![
        Constraint::Length(20),
        Constraint::Length(18),
        Constraint::Fill(1),
    ];

    let table = ThemedTable {
        headers: vec!["Timestamp", "Author", "Message"],
        rows,
        selected: Some(activities_model.selected_row),
        widths,
        page_info,
    };

    table.render(area, buf, ctx);
}

/// Build the table rows for the current page (newest-first).
fn build_table_rows(
    data: &TicketActivitiesData,
    current_page: usize,
    page_size: usize,
) -> Vec<Vec<String>> {
    let reversed: Vec<_> = data.activities.iter().rev().collect();
    let start = current_page * page_size;
    let end = (start + page_size).min(reversed.len());
    if start >= reversed.len() {
        return vec![];
    }

    reversed[start..end]
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

/// Format a timestamp string to `YYYY-MM-DD HH:MM:SS` (19 chars).
fn format_timestamp(ts: &str) -> String {
    let s = ts.replace('T', " ");
    let trimmed = s
        .trim_end_matches('Z')
        .trim_end_matches(|c: char| !c.is_ascii_digit() && c != ' ' && c != '-' && c != ':');
    let candidate: String = trimmed.chars().take(19).collect();
    if candidate.len() == 19 {
        candidate
    } else {
        format!("{:<19}", candidate)
    }
}

/// Truncate a string to at most `max_chars` Unicode characters.
fn truncate(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{truncated}...")
    }
}

/// Handle a `DataMsg::ActivitiesLoaded` for the activities page.
///
/// Updates the activities model with loaded data and rebuilds the author list.
pub fn handle_activities_data(
    model: &mut Model,
    ticket_id: String,
    result: Result<Vec<ur_rpc::proto::ticket::ActivityEntry>, String>,
) {
    let Some(ref mut activities_model) = model.ticket_activities else {
        return;
    };
    if activities_model.ticket_id != ticket_id {
        return;
    }

    match result {
        Ok(activities) => {
            // Rebuild authors when unfiltered (author_index == 0).
            if activities_model.author_index == 0 {
                activities_model.authors = rebuild_authors(&activities);
            }
            activities_model.data = LoadState::Loaded(TicketActivitiesData { activities });
            clamp_selection(activities_model);
        }
        Err(e) => {
            activities_model.data = LoadState::Error(e);
        }
    }
}

/// Rebuild the unique author list from activities.
/// Index 0 is always "all". Authors appear in insertion order.
fn rebuild_authors(activities: &[ur_rpc::proto::ticket::ActivityEntry]) -> Vec<String> {
    let mut seen = Vec::new();
    seen.push("all".to_string());
    for entry in activities {
        if !entry.author.is_empty() && !seen.contains(&entry.author) {
            seen.push(entry.author.clone());
        }
    }
    seen
}

/// Clamp the selected row to valid bounds after data changes.
fn clamp_selection(activities_model: &mut super::super::model::TicketActivitiesModel) {
    let total = activities_model
        .data
        .data()
        .map(|d| d.activities.len())
        .unwrap_or(0);
    let page_size = activities_model.page_size;
    let current_page = activities_model.current_page;

    let reversed_len = total;
    let start = current_page * page_size;
    let page_count = if start >= reversed_len {
        0
    } else {
        (start + page_size).min(reversed_len) - start
    };

    if page_count == 0 {
        activities_model.selected_row = 0;
    } else if activities_model.selected_row >= page_count {
        activities_model.selected_row = page_count.saturating_sub(1);
    }
}

/// Start fetching activities for a ticket. Sets up the model and returns the fetch command.
pub fn start_activities_fetch(model: &mut Model, ticket_id: String, ticket_title: String) -> Cmd {
    model.ticket_activities = Some(super::super::model::TicketActivitiesModel {
        ticket_id: ticket_id.clone(),
        ticket_title,
        data: LoadState::Loading,
        selected_row: 0,
        current_page: 0,
        page_size: 20,
        authors: vec!["all".to_string()],
        author_index: 0,
    });
    Cmd::Fetch(FetchCmd::Activities {
        ticket_id,
        author_filter: None,
    })
}

/// Input handler for the ticket activities page.
///
/// Handles scroll navigation (j/k, arrow keys), pagination (h/l),
/// author filter cycling (f), refresh (r), and back (Esc handled by GlobalHandler).
pub struct TicketActivitiesHandler;

impl InputHandler for TicketActivitiesHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        match (key.code, key.modifiers) {
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::ActivitiesNavigate { delta: 1 }))
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::ActivitiesNavigate { delta: -1 }))
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::ActivitiesPageRight))
            }
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::ActivitiesPageLeft))
            }
            (KeyCode::Char('f'), KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::ActivitiesCycleFilter))
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::ActivitiesRefresh))
            }
            _ => InputResult::Bubble,
        }
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "f".to_string(),
                description: "Filter author".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "j/k".to_string(),
                description: "Scroll".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: "h/l".to_string(),
                description: "Page".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: "r".to_string(),
                description: "Refresh".to_string(),
                common: true,
            },
        ]
    }

    fn name(&self) -> &str {
        "ticket_activities"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use ur_rpc::proto::ticket::ActivityEntry;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn make_entry(author: &str, timestamp: &str, message: &str) -> ActivityEntry {
        ActivityEntry {
            id: format!("act-{author}"),
            timestamp: timestamp.to_string(),
            author: author.to_string(),
            message: message.to_string(),
        }
    }

    // ── format_timestamp ───────────────────────────────────────────────

    #[test]
    fn format_timestamp_iso8601_with_z() {
        let ts = format_timestamp("2026-03-25T14:32:10Z");
        assert_eq!(ts, "2026-03-25 14:32:10");
    }

    #[test]
    fn format_timestamp_iso8601_with_subseconds() {
        let ts = format_timestamp("2026-03-25T14:32:10.123456Z");
        assert_eq!(ts, "2026-03-25 14:32:10");
    }

    // ── truncate ───────────────────────────────────────────────────────

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string_adds_ellipsis() {
        let s = truncate("hello world this is long", 10);
        assert!(s.len() <= 12); // 9 chars + "..."
    }

    // ── rebuild_authors ────────────────────────────────────────────────

    #[test]
    fn rebuild_authors_deduplicates() {
        let entries = vec![
            make_entry("alice", "", ""),
            make_entry("bob", "", ""),
            make_entry("alice", "", ""),
        ];
        let authors = rebuild_authors(&entries);
        assert_eq!(authors, vec!["all", "alice", "bob"]);
    }

    #[test]
    fn rebuild_authors_empty_returns_only_all() {
        let authors = rebuild_authors(&[]);
        assert_eq!(authors, vec!["all"]);
    }

    // ── build_table_rows ───────────────────────────────────────────────

    #[test]
    fn build_table_rows_newest_first() {
        let data = TicketActivitiesData {
            activities: vec![
                make_entry("alice", "2026-01-01T00:00:00Z", "oldest"),
                make_entry("bob", "2026-01-02T00:00:00Z", "newest"),
            ],
        };
        let rows = build_table_rows(&data, 0, 20);
        assert_eq!(rows.len(), 2);
        assert!(rows[0][2].contains("newest"));
        assert!(rows[1][2].contains("oldest"));
    }

    #[test]
    fn build_table_rows_pagination() {
        let data = TicketActivitiesData {
            activities: vec![
                make_entry("a", "2026-01-01T00:00:00Z", "msg0"),
                make_entry("a", "2026-01-01T00:00:00Z", "msg1"),
                make_entry("a", "2026-01-01T00:00:00Z", "msg2"),
                make_entry("a", "2026-01-01T00:00:00Z", "msg3"),
            ],
        };
        let rows_page0 = build_table_rows(&data, 0, 2);
        assert_eq!(rows_page0.len(), 2);
        let rows_page1 = build_table_rows(&data, 1, 2);
        assert_eq!(rows_page1.len(), 2);
    }

    // ── input handler ──────────────────────────────────────────────────

    #[test]
    fn handler_j_captures_navigate_down() {
        let handler = TicketActivitiesHandler;
        let key = make_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_k_captures_navigate_up() {
        let handler = TicketActivitiesHandler;
        let key = make_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_f_captures_filter() {
        let handler = TicketActivitiesHandler;
        let key = make_key(KeyCode::Char('f'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_unknown_bubbles() {
        let handler = TicketActivitiesHandler;
        let key = make_key(KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn handler_footer_has_filter_author() {
        let handler = TicketActivitiesHandler;
        let commands = handler.footer_commands();
        assert!(commands.iter().any(|c| c.description == "Filter author"));
    }

    #[test]
    fn handler_name() {
        let handler = TicketActivitiesHandler;
        assert_eq!(handler.name(), "ticket_activities");
    }

    // ── handle_activities_data ─────────────────────────────────────────

    #[test]
    fn handle_data_ok_loads_activities() {
        let mut model = Model::initial();
        model.ticket_activities = Some(super::super::super::model::TicketActivitiesModel {
            ticket_id: "ur-test".to_string(),
            ticket_title: "Test".to_string(),
            data: LoadState::Loading,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
            authors: vec!["all".to_string()],
            author_index: 0,
        });

        let entries = vec![make_entry("alice", "2026-01-01T00:00:00Z", "hello")];
        handle_activities_data(&mut model, "ur-test".to_string(), Ok(entries));

        let am = model.ticket_activities.unwrap();
        assert!(am.data.is_loaded());
        assert_eq!(am.authors.len(), 2); // all + alice
    }

    #[test]
    fn handle_data_error_sets_error_state() {
        let mut model = Model::initial();
        model.ticket_activities = Some(super::super::super::model::TicketActivitiesModel {
            ticket_id: "ur-test".to_string(),
            ticket_title: "Test".to_string(),
            data: LoadState::Loading,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
            authors: vec!["all".to_string()],
            author_index: 0,
        });

        handle_activities_data(
            &mut model,
            "ur-test".to_string(),
            Err("rpc failed".to_string()),
        );

        let am = model.ticket_activities.unwrap();
        assert!(matches!(am.data, LoadState::Error(_)));
    }

    #[test]
    fn handle_data_ignores_mismatched_ticket_id() {
        let mut model = Model::initial();
        model.ticket_activities = Some(super::super::super::model::TicketActivitiesModel {
            ticket_id: "ur-test".to_string(),
            ticket_title: "Test".to_string(),
            data: LoadState::Loading,
            selected_row: 0,
            current_page: 0,
            page_size: 20,
            authors: vec!["all".to_string()],
            author_index: 0,
        });

        handle_activities_data(&mut model, "ur-other".to_string(), Ok(vec![]));

        let am = model.ticket_activities.unwrap();
        assert!(am.data.is_loading());
    }

    // ── start_activities_fetch ─────────────────────────────────────────

    #[test]
    fn start_fetch_creates_model_and_cmd() {
        let mut model = Model::initial();
        let cmd = start_activities_fetch(&mut model, "ur-abc".to_string(), "Title".to_string());
        assert!(model.ticket_activities.is_some());
        let am = model.ticket_activities.as_ref().unwrap();
        assert_eq!(am.ticket_id, "ur-abc");
        assert!(am.data.is_loading());
        assert!(matches!(cmd, Cmd::Fetch(FetchCmd::Activities { .. })));
    }
}
