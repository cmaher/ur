use chrono::{DateTime, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use ur_rpc::lifecycle;
use ur_rpc::proto::ticket::WorkflowInfo;

use crate::context::TuiContext;
use crate::data::DataPayload;
use crate::keymap::{Action, Keymap};
use crate::page::FooterCommand;
use crate::screen::{Screen, ScreenResult};
use crate::widgets::ThemedTable;

/// Screen showing the full detail view for a single workflow.
///
/// Pushed onto the Flows tab stack when the user selects a workflow in
/// `FlowsListScreen`. Pressing Back/Escape pops back to the list.
pub struct FlowDetailScreen {
    workflow: WorkflowInfo,
}

impl FlowDetailScreen {
    pub fn new(workflow: WorkflowInfo) -> Self {
        Self { workflow }
    }
}

impl Screen for FlowDetailScreen {
    fn handle_action(&mut self, action: Action) -> ScreenResult {
        match action {
            Action::Back => ScreenResult::Pop,
            Action::Quit => ScreenResult::Quit,
            _ => ScreenResult::Consumed,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        render_flow_detail(&self.workflow, area, buf, ctx);
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        detail_footer_commands(keymap)
    }

    fn on_data(&mut self, _payload: &DataPayload) {}

    fn needs_data(&self) -> bool {
        false
    }

    fn mark_stale(&mut self) {}
}

/// Render the full detail view for a single workflow.
pub fn render_flow_detail(wf: &WorkflowInfo, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let chunks =
        Layout::vertical([Constraint::Length(field_line_count()), Constraint::Min(3)]).split(area);

    render_field_list(wf, chunks[0], buf, ctx);
    render_history_table(wf, chunks[1], buf, ctx);
}

/// Returns the number of lines needed for the field list.
fn field_line_count() -> u16 {
    // ticket_id, workflow_id, status, feedback_mode, worker_id,
    // implement_cycles, pr_url, children, created_at, stall_info, blank separator
    11
}

/// Render WorkflowInfo fields as label-value pairs.
fn render_field_list(wf: &WorkflowInfo, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let dim = Style::default().fg(ctx.theme.neutral_content);
    let normal = Style::default().fg(ctx.theme.base_content);
    let error_style = Style::default().fg(ctx.theme.error);

    let children_total = wf.ticket_children_open + wf.ticket_children_closed;
    let children_text = format!(
        "{} closed / {} total",
        wf.ticket_children_closed, children_total
    );

    let stall_line = if wf.stalled {
        Line::from(vec![
            Span::styled("Stall Reason:   ", dim),
            Span::styled(&wf.stall_reason, error_style),
        ])
    } else {
        Line::from(vec![
            Span::styled("Stalled:        ", dim),
            Span::styled("No", normal),
        ])
    };

    let lines = vec![
        Line::from(vec![
            Span::styled("Ticket ID:      ", dim),
            Span::styled(&wf.ticket_id, normal),
        ]),
        Line::from(vec![
            Span::styled("Workflow ID:    ", dim),
            Span::styled(&wf.id, normal),
        ]),
        Line::from(vec![
            Span::styled("Status:         ", dim),
            Span::styled(&wf.status, normal),
        ]),
        Line::from(vec![
            Span::styled("Feedback Mode:  ", dim),
            Span::styled(&wf.feedback_mode, normal),
        ]),
        Line::from(vec![
            Span::styled("Worker ID:      ", dim),
            Span::styled(&wf.worker_id, normal),
        ]),
        Line::from(vec![
            Span::styled("Impl Cycles:    ", dim),
            Span::styled(wf.implement_cycles.to_string(), normal),
        ]),
        Line::from(vec![
            Span::styled("PR URL:         ", dim),
            Span::styled(&wf.pr_url, normal),
        ]),
        Line::from(vec![
            Span::styled("Children:       ", dim),
            Span::styled(children_text, normal),
        ]),
        Line::from(vec![
            Span::styled("Created At:     ", dim),
            Span::styled(&wf.created_at, normal),
        ]),
        stall_line,
        Line::raw(""),
    ];

    let paragraph = Paragraph::new(lines);
    paragraph.render(area, buf);
}

/// Check if a workflow status is terminal (times should be frozen).
fn is_terminal_status(status: &str) -> bool {
    status == lifecycle::DONE || status == lifecycle::CANCELLED
}

/// Format a chrono Duration as HH:MM:SS.
fn format_duration_hhmmss(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds().max(0);
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Render the history table with Event, Timestamp, and Duration columns.
fn render_history_table(wf: &WorkflowInfo, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let now = Utc::now();
    let terminal = is_terminal_status(&wf.status);

    let rows: Vec<Vec<String>> = build_history_rows(wf, now, terminal);

    let widths = vec![
        Constraint::Length(25), // Event
        Constraint::Length(22), // Timestamp
        Constraint::Fill(1),    // Duration
    ];

    let table = ThemedTable {
        headers: vec!["Event", "Timestamp", "Duration"],
        rows,
        selected: None,
        widths,
        page_info: None,
    };

    table.render(area, buf, ctx);
}

/// Build rows for the history table from consecutive event pairs.
fn build_history_rows(wf: &WorkflowInfo, now: DateTime<Utc>, terminal: bool) -> Vec<Vec<String>> {
    let history = &wf.history;
    let len = history.len();
    let mut rows = Vec::with_capacity(len);

    for (i, evt) in history.iter().enumerate() {
        let duration_str = compute_event_duration(history, i, now, terminal);
        rows.push(vec![
            evt.event.clone(),
            format_timestamp(&evt.created_at),
            duration_str,
        ]);
    }

    // Reverse so newest events appear first
    if len > 1 {
        rows.reverse();
    }

    rows
}

/// Compute the duration string for a history event at position `idx`.
///
/// For events that have a successor, duration = next.created_at - this.created_at.
/// For the last event in a non-terminal workflow, duration = now - this.created_at (live elapsed).
/// For the last event in a terminal workflow, duration is shown as "-".
fn compute_event_duration(
    history: &[ur_rpc::proto::ticket::WorkflowHistoryEvent],
    idx: usize,
    now: DateTime<Utc>,
    terminal: bool,
) -> String {
    let current_ts = parse_rfc3339(&history[idx].created_at);

    if idx + 1 < history.len() {
        // Duration to the next event
        let next_ts = parse_rfc3339(&history[idx + 1].created_at);
        match (current_ts, next_ts) {
            (Some(cur), Some(nxt)) => format_duration_hhmmss(nxt - cur),
            _ => "-".to_string(),
        }
    } else if !terminal {
        // Last event, non-terminal: show live elapsed
        match current_ts {
            Some(cur) => format_duration_hhmmss(now - cur),
            None => "-".to_string(),
        }
    } else {
        // Last event, terminal: no live duration
        "-".to_string()
    }
}

/// Parse an RFC3339 timestamp string into a DateTime<Utc>.
fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Format a timestamp string for display. Shows the original if parseable, otherwise as-is.
fn format_timestamp(ts: &str) -> String {
    match parse_rfc3339(ts) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => ts.to_string(),
    }
}

/// Returns the footer commands for the detail view.
pub fn detail_footer_commands(keymap: &Keymap) -> Vec<FooterCommand> {
    vec![
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

#[cfg(test)]
mod tests {
    use super::*;
    use ur_rpc::proto::ticket::WorkflowHistoryEvent;

    fn make_event(event: &str, created_at: &str) -> WorkflowHistoryEvent {
        WorkflowHistoryEvent {
            event: event.into(),
            created_at: created_at.into(),
        }
    }

    fn make_workflow(history: Vec<WorkflowHistoryEvent>, status: &str) -> WorkflowInfo {
        WorkflowInfo {
            id: "wf-123".into(),
            ticket_id: "ur-abc".into(),
            status: status.into(),
            stalled: false,
            stall_reason: String::new(),
            implement_cycles: 2,
            worker_id: "w-1".into(),
            feedback_mode: "auto".into(),
            created_at: "2026-03-22T10:00:00+00:00".into(),
            pr_url: "https://github.com/org/repo/pull/42".into(),
            history,
            ticket_children_open: 3,
            ticket_children_closed: 7,
        }
    }

    #[test]
    fn format_duration_basic() {
        let d = chrono::Duration::seconds(3661);
        assert_eq!(format_duration_hhmmss(d), "01:01:01");
    }

    #[test]
    fn format_duration_zero() {
        let d = chrono::Duration::seconds(0);
        assert_eq!(format_duration_hhmmss(d), "00:00:00");
    }

    #[test]
    fn format_duration_negative_clamps() {
        let d = chrono::Duration::seconds(-10);
        assert_eq!(format_duration_hhmmss(d), "00:00:00");
    }

    #[test]
    fn format_timestamp_valid() {
        let ts = "2026-03-22T10:30:00+00:00";
        assert_eq!(format_timestamp(ts), "2026-03-22 10:30:00");
    }

    #[test]
    fn format_timestamp_invalid_returns_original() {
        let ts = "not-a-date";
        assert_eq!(format_timestamp(ts), "not-a-date");
    }

    #[test]
    fn duration_between_consecutive_events() {
        let history = vec![
            make_event("started", "2026-03-22T10:00:00+00:00"),
            make_event("implementing", "2026-03-22T11:30:00+00:00"),
        ];
        let now = Utc::now();
        let dur = compute_event_duration(&history, 0, now, false);
        assert_eq!(dur, "01:30:00");
    }

    #[test]
    fn last_event_terminal_shows_dash() {
        let history = vec![
            make_event("started", "2026-03-22T10:00:00+00:00"),
            make_event("done", "2026-03-22T11:00:00+00:00"),
        ];
        let now = Utc::now();
        let dur = compute_event_duration(&history, 1, now, true);
        assert_eq!(dur, "-");
    }

    #[test]
    fn last_event_non_terminal_shows_live_elapsed() {
        let history = vec![make_event("implementing", "2026-03-22T10:00:00+00:00")];
        let now = DateTime::parse_from_rfc3339("2026-03-22T10:05:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let dur = compute_event_duration(&history, 0, now, false);
        assert_eq!(dur, "00:05:00");
    }

    #[test]
    fn build_history_rows_reverses_order() {
        let history = vec![
            make_event("started", "2026-03-22T10:00:00+00:00"),
            make_event("implementing", "2026-03-22T11:00:00+00:00"),
            make_event("done", "2026-03-22T12:00:00+00:00"),
        ];
        let wf = make_workflow(history, "done");
        let now = Utc::now();
        let rows = build_history_rows(&wf, now, true);
        // Newest first
        assert_eq!(rows[0][0], "done");
        assert_eq!(rows[1][0], "implementing");
        assert_eq!(rows[2][0], "started");
    }

    #[test]
    fn build_history_rows_empty() {
        let wf = make_workflow(vec![], "open");
        let now = Utc::now();
        let rows = build_history_rows(&wf, now, false);
        assert!(rows.is_empty());
    }

    #[test]
    fn detail_footer_has_back_and_quit() {
        let keymap = Keymap::default();
        let cmds = detail_footer_commands(&keymap);
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].description, "Back");
        assert_eq!(cmds[1].description, "Quit");
    }

    #[test]
    fn is_terminal_done() {
        assert!(is_terminal_status("done"));
    }

    #[test]
    fn is_terminal_cancelled() {
        assert!(is_terminal_status("cancelled"));
    }

    #[test]
    fn is_not_terminal_implementing() {
        assert!(!is_terminal_status("implementing"));
    }
}
