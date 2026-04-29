use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use ur_rpc::lifecycle;
use ur_rpc::proto::ticket::WorkflowInfo;

use crate::cmd::Cmd;
use crate::components::{ThemedTable, format_pr_short, render_hyperlink};
use crate::context::TuiContext;
use crate::input::{FooterCommand, InputHandler, InputResult};
use crate::model::{FlowDetailModel, Model};
use crate::msg::{FlowOpMsg, GotoTarget, Msg, NavMsg, OverlayMsg, TicketOpMsg};

/// Initialize the flow detail model for a given ticket.
///
/// First checks the already-loaded flow list data. If the workflow is found
/// there, populates the model immediately. Otherwise, issues a gRPC fetch
/// to load the workflow by ticket ID.
pub fn init_flow_detail(model: &mut Model, ticket_id: String) -> Vec<Cmd> {
    if let Some(data) = model.flow_list.data.data()
        && let Some(wf) = data.workflows.iter().find(|w| w.ticket_id == ticket_id)
    {
        model.flow_detail = Some(FlowDetailModel {
            ticket_id: ticket_id.clone(),
            workflow: wf.clone(),
        });
        vec![]
    } else {
        // Flow list not loaded or workflow not in current page — fetch directly.
        model.flow_detail = None;
        vec![Cmd::Fetch(crate::cmd::FetchCmd::FlowDetail { ticket_id })]
    }
}

/// Render the flow detail page into the given content area.
pub fn render_flow_detail(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    match &model.flow_detail {
        Some(detail) => {
            render_flow_detail_content(&detail.workflow, area, buf, ctx);
        }
        None => {
            let style = Style::default()
                .fg(ctx.theme.base_content)
                .bg(ctx.theme.base_100);
            Paragraph::new(Line::raw("Flow not found"))
                .style(style)
                .render(area, buf);
        }
    }
}

/// Render the full detail view for a single workflow.
fn render_flow_detail_content(wf: &WorkflowInfo, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let chunks =
        Layout::vertical([Constraint::Length(field_line_count()), Constraint::Min(3)]).split(area);

    render_field_list(wf, chunks[0], buf, ctx);
    render_history_table(wf, chunks[1], buf, ctx);
}

/// Returns the number of lines needed for the field list.
fn field_line_count() -> u16 {
    // ticket_id, ticket_title, workflow_id, status, feedback_mode, worker_id,
    // implement_cycles, pr_url, children, created_at, stall_info, blank separator
    12
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
            Span::styled("Ticket Title:   ", dim),
            Span::styled(&wf.ticket_title, normal),
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

    render_pr_url_hyperlink(wf, area, buf, ctx);
}

/// The number of characters in the PR URL field label (used to find the value x-offset).
const PR_URL_LABEL_LEN: u16 = 16; // "PR URL:         "

/// The 0-based row index of the PR URL line within the field list.
const PR_URL_ROW: u16 = 7;

/// Overlay a clickable OSC 8 hyperlink on the PR URL field value.
///
/// This is called after the paragraph renders so it can overwrite the plain
/// `Span::styled` text with the hyperlink-decorated cells.
fn render_pr_url_hyperlink(wf: &WorkflowInfo, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    if wf.pr_url.is_empty() {
        return;
    }

    let y = area.y + PR_URL_ROW;
    if y >= area.y + area.height {
        return;
    }

    let value_x = area.x + PR_URL_LABEL_LEN;
    let available_width = area.width.saturating_sub(PR_URL_LABEL_LEN);
    if available_width == 0 {
        return;
    }

    let display = format_pr_short(&wf.pr_url).unwrap_or_else(|| wf.pr_url.clone());
    let fg = ctx.theme.base_content;
    let bg = ctx.theme.base_100;

    let cell = Rect {
        x: value_x,
        y,
        width: available_width,
        height: 1,
    };

    render_hyperlink(cell, buf, &wf.pr_url, &display, fg, bg);
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

/// Parse an RFC3339 timestamp string into a DateTime<Utc>.
fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Format a timestamp string for display.
fn format_timestamp(ts: &str) -> String {
    match parse_rfc3339(ts) {
        Some(dt) => dt.format("%Y-%m-%d %H:%M:%S").to_string(),
        None => ts.to_string(),
    }
}

/// Render the history table with Event, Timestamp, and Duration columns.
fn render_history_table(wf: &WorkflowInfo, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
    let now = Utc::now();
    let terminal = is_terminal_status(&wf.status);
    let rows = build_history_rows(wf, now, terminal);

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
fn compute_event_duration(
    history: &[ur_rpc::proto::ticket::WorkflowHistoryEvent],
    idx: usize,
    now: DateTime<Utc>,
    terminal: bool,
) -> String {
    let current_ts = parse_rfc3339(&history[idx].created_at);

    if idx + 1 < history.len() {
        let next_ts = parse_rfc3339(&history[idx + 1].created_at);
        match (current_ts, next_ts) {
            (Some(cur), Some(nxt)) => format_duration_hhmmss(nxt - cur),
            _ => "-".to_string(),
        }
    } else if !terminal {
        match current_ts {
            Some(cur) => format_duration_hhmmss(now - cur),
            None => "-".to_string(),
        }
    } else {
        "-".to_string()
    }
}

/// Handle flow detail navigation messages.
pub fn handle_flow_detail_nav(model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    match nav_msg {
        NavMsg::FlowDetailCancel => handle_cancel(model),
        NavMsg::FlowDetailApprove => handle_approve(model),
        NavMsg::FlowDetailRedrive => handle_redrive(model),
        NavMsg::FlowDetailGoto => handle_goto(model),
        _ => (model, vec![]),
    }
}

/// Cancel the workflow shown in flow detail.
fn handle_cancel(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ref detail) = model.flow_detail {
        let msg = Msg::FlowOp(FlowOpMsg::Cancel {
            ticket_id: detail.ticket_id.clone(),
        });
        crate::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Approve the workflow shown in flow detail.
fn handle_approve(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ref detail) = model.flow_detail {
        let msg = Msg::FlowOp(FlowOpMsg::Approve {
            ticket_id: detail.ticket_id.clone(),
        });
        crate::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Redrive the workflow shown in flow detail.
fn handle_redrive(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ref detail) = model.flow_detail {
        let msg = Msg::TicketOp(TicketOpMsg::Redrive {
            ticket_id: detail.ticket_id.clone(),
        });
        crate::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Open the goto menu from flow detail.
fn handle_goto(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(ref detail) = model.flow_detail {
        let targets = build_flow_detail_goto_targets(&detail.ticket_id);
        let msg = Msg::Overlay(OverlayMsg::OpenGotoMenu { targets });
        crate::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Build goto targets for a flow detail view.
fn build_flow_detail_goto_targets(ticket_id: &str) -> Vec<GotoTarget> {
    vec![
        GotoTarget {
            label: "Ticket Details".to_string(),
            screen: "ticket".to_string(),
            id: ticket_id.to_string(),
        },
        GotoTarget {
            label: "Worker".to_string(),
            screen: "worker".to_string(),
            id: ticket_id.to_string(),
        },
    ]
}

/// Input handler for the flow detail page.
pub struct FlowDetailHandler;

impl InputHandler for FlowDetailHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        if key.modifiers.contains(KeyModifiers::SHIFT)
            && let Some(msg) = handle_shift_key(key.code)
        {
            return InputResult::Capture(msg);
        }

        if key.modifiers == KeyModifiers::NONE
            && let Some(msg) = handle_plain_key(key.code)
        {
            return InputResult::Capture(msg);
        }

        InputResult::Bubble
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "A".to_string(),
                description: "Approve".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "V".to_string(),
                description: "Redrive".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "X".to_string(),
                description: "Cancel".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "g".to_string(),
                description: "Goto".to_string(),
                common: false,
            },
        ]
    }

    fn name(&self) -> &str {
        "flow_detail"
    }
}

/// Handle Shift+letter keys on the flow detail page.
fn handle_shift_key(code: KeyCode) -> Option<Msg> {
    match code {
        KeyCode::Char('A') => Some(Msg::Nav(NavMsg::FlowDetailApprove)),
        KeyCode::Char('X') => Some(Msg::Nav(NavMsg::FlowDetailCancel)),
        KeyCode::Char('V') => Some(Msg::Nav(NavMsg::FlowDetailRedrive)),
        _ => None,
    }
}

/// Handle plain keys on the flow detail page.
fn handle_plain_key(code: KeyCode) -> Option<Msg> {
    match code {
        KeyCode::Char('g') => Some(Msg::Nav(NavMsg::FlowDetailGoto)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};
    use ur_rpc::proto::ticket::WorkflowHistoryEvent;

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

    fn make_workflow(status: &str) -> WorkflowInfo {
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
            history: vec![],
            ticket_children_open: 3,
            ticket_children_closed: 7,
            ticket_title: "Test ticket".into(),
        }
    }

    fn make_event(event: &str, created_at: &str) -> WorkflowHistoryEvent {
        WorkflowHistoryEvent {
            event: event.into(),
            created_at: created_at.into(),
        }
    }

    // ── format_duration ─────────────────────────────────────────────

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

    // ── format_timestamp ────────────────────────────────────────────

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

    // ── is_terminal_status ──────────────────────────────────────────

    #[test]
    fn done_is_terminal() {
        assert!(is_terminal_status("done"));
    }

    #[test]
    fn cancelled_is_terminal() {
        assert!(is_terminal_status("cancelled"));
    }

    #[test]
    fn implementing_is_not_terminal() {
        assert!(!is_terminal_status("implementing"));
    }

    // ── compute_event_duration ──────────────────────────────────────

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

    // ── build_history_rows ──────────────────────────────────────────

    #[test]
    fn build_history_rows_reverses_order() {
        let mut wf = make_workflow("done");
        wf.history = vec![
            make_event("started", "2026-03-22T10:00:00+00:00"),
            make_event("implementing", "2026-03-22T11:00:00+00:00"),
            make_event("done", "2026-03-22T12:00:00+00:00"),
        ];
        let now = Utc::now();
        let rows = build_history_rows(&wf, now, true);
        assert_eq!(rows[0][0], "done");
        assert_eq!(rows[1][0], "implementing");
        assert_eq!(rows[2][0], "started");
    }

    #[test]
    fn build_history_rows_empty() {
        let wf = make_workflow("open");
        let now = Utc::now();
        let rows = build_history_rows(&wf, now, false);
        assert!(rows.is_empty());
    }

    // ── input handler ───────────────────────────────────────────────

    #[test]
    fn handler_shift_a_captures_approve() {
        let handler = FlowDetailHandler;
        let key = make_key(KeyCode::Char('A'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowDetailApprove)) => {}
            other => panic!("expected approve, got {other:?}"),
        }
    }

    #[test]
    fn handler_shift_x_captures_cancel() {
        let handler = FlowDetailHandler;
        let key = make_key(KeyCode::Char('X'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowDetailCancel)) => {}
            other => panic!("expected cancel, got {other:?}"),
        }
    }

    #[test]
    fn handler_shift_v_captures_redrive() {
        let handler = FlowDetailHandler;
        let key = make_key(KeyCode::Char('V'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowDetailRedrive)) => {}
            other => panic!("expected redrive, got {other:?}"),
        }
    }

    #[test]
    fn handler_g_captures_goto() {
        let handler = FlowDetailHandler;
        match handler.handle_key(plain_key(KeyCode::Char('g'))) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowDetailGoto)) => {}
            other => panic!("expected goto, got {other:?}"),
        }
    }

    #[test]
    fn handler_bubbles_unrecognized() {
        let handler = FlowDetailHandler;
        assert!(matches!(
            handler.handle_key(plain_key(KeyCode::Char('z'))),
            InputResult::Bubble
        ));
    }

    #[test]
    fn handler_footer_has_expected_commands() {
        let handler = FlowDetailHandler;
        let cmds = handler.footer_commands();
        assert_eq!(cmds.len(), 4);
        assert!(cmds.iter().any(|c| c.description == "Approve"));
        assert!(cmds.iter().any(|c| c.description == "Redrive"));
        assert!(cmds.iter().any(|c| c.description == "Cancel"));
        assert!(cmds.iter().any(|c| c.description == "Goto"));
    }

    #[test]
    fn handler_footer_approve_comes_first() {
        let handler = FlowDetailHandler;
        let cmds = handler.footer_commands();
        assert_eq!(cmds[0].key_label, "A");
        assert_eq!(cmds[0].description, "Approve");
    }

    #[test]
    fn handler_name() {
        let handler = FlowDetailHandler;
        assert_eq!(handler.name(), "flow_detail");
    }

    // ── goto targets ────────────────────────────────────────────────

    #[test]
    fn goto_targets_include_standard_options() {
        let targets = build_flow_detail_goto_targets("ur-abc");
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|t| t.label == "Ticket Details"));
        assert!(targets.iter().any(|t| t.label == "Worker"));
    }

    // ── init_flow_detail ────────────────────────────────────────────

    #[test]
    fn init_flow_detail_sets_model() {
        use crate::model::{FlowListData, LoadState};
        let mut model = Model::initial();
        let wf = make_workflow("implementing");
        model.flow_list.data = LoadState::Loaded(FlowListData {
            workflows: vec![wf.clone()],
            total_count: 1,
        });

        let cmds = init_flow_detail(&mut model, "ur-abc".to_string());

        let detail = model.flow_detail.as_ref().unwrap();
        assert_eq!(detail.ticket_id, "ur-abc");
        assert_eq!(detail.workflow.id, "wf-123");
        assert!(cmds.is_empty(), "should not fetch when found in cache");
    }

    #[test]
    fn init_flow_detail_not_found_issues_fetch() {
        use crate::model::LoadState;
        let mut model = Model::initial();
        model.flow_list.data = LoadState::Loaded(crate::model::FlowListData {
            workflows: vec![],
            total_count: 0,
        });

        let cmds = init_flow_detail(&mut model, "ur-missing".to_string());
        assert!(model.flow_detail.is_none());
        assert_eq!(cmds.len(), 1, "should issue a fetch command");
        assert!(
            matches!(&cmds[0], Cmd::Fetch(crate::cmd::FetchCmd::FlowDetail { ticket_id }) if ticket_id == "ur-missing"),
            "should fetch the specific ticket"
        );
    }

    #[test]
    fn init_flow_detail_unloaded_issues_fetch() {
        let mut model = Model::initial();
        // flow_list.data is NotLoaded by default
        let cmds = init_flow_detail(&mut model, "ur-xyz".to_string());
        assert!(model.flow_detail.is_none());
        assert_eq!(cmds.len(), 1);
    }

    // ── render_pr_url_hyperlink ─────────────────────────────────────

    fn make_ctx() -> crate::context::TuiContext {
        use crate::keymap::Keymap;
        use crate::theme::Theme;
        use ur_config::TuiConfig;
        let tui_config = TuiConfig::default();
        let theme = Theme::resolve(&tui_config);
        crate::context::TuiContext {
            theme,
            keymap: Keymap::default(),
            projects: vec![],
            project_configs: std::collections::HashMap::new(),
            tui_config,
            config_dir: std::path::PathBuf::from("/tmp/test-urui"),
            project_filter: None,
        }
    }

    /// Render the PR URL overlay into a buffer and return the display characters
    /// for the value area of the PR URL row (columns after the 16-char label).
    ///
    /// Each cell's symbol may contain OSC 8 escape sequences; this extracts the
    /// visible character by stripping leading ESC sequences and taking the first
    /// non-ESC, non-BEL character.
    fn render_pr_hyperlink_display(wf: &WorkflowInfo, width: u16) -> String {
        let area = ratatui::layout::Rect::new(0, 0, width, field_line_count());
        let mut buf = ratatui::buffer::Buffer::empty(area);
        let ctx = make_ctx();
        render_pr_url_hyperlink(wf, area, &mut buf, &ctx);
        // Value area starts at x = PR_URL_LABEL_LEN, y = PR_URL_ROW.
        // Strip OSC 8 escape sequences to get just the visible characters.
        (PR_URL_LABEL_LEN..width)
            .map(|x| {
                let sym = buf[(x, PR_URL_ROW)].symbol().to_string();
                // OSC 8 cells look like: ESC]8;;URL BEL char ESC]8;; BEL
                // Extract visible chars: those that are not ESC, BEL, or in ESC sequences.
                extract_visible_char(&sym)
            })
            .collect()
    }

    /// Extract the single visible character from a cell symbol that may contain
    /// OSC 8 sequences (`ESC]8;;URL BEL char ESC]8;; BEL`).
    fn extract_visible_char(sym: &str) -> char {
        // Walk through the string, skipping ESC sequences and BEL characters.
        // An OSC sequence starts at ESC (0x1b) and ends at BEL (0x07) or ST (ESC\\).
        let mut in_esc = false;
        let mut chars = sym.chars();
        while let Some(c) = chars.next() {
            match c {
                '\x1b' => {
                    in_esc = true;
                }
                '\x07' => {
                    // BEL terminates an OSC sequence; the next char is the visible one.
                    in_esc = false;
                }
                _ if in_esc => {
                    // Still inside an escape sequence; keep skipping.
                }
                _ => {
                    return c;
                }
            }
        }
        ' '
    }

    #[test]
    fn pr_url_empty_renders_nothing() {
        let mut wf = make_workflow("done");
        wf.pr_url = String::new();
        // An empty pr_url should leave the value area untouched (all spaces).
        let display = render_pr_hyperlink_display(&wf, 80);
        let all_spaces = display.chars().all(|c| c == ' ');
        assert!(
            all_spaces,
            "expected all spaces for empty pr_url, got: {display:?}"
        );
    }

    #[test]
    fn pr_url_github_displays_short_format() {
        let mut wf = make_workflow("done");
        wf.pr_url = "https://github.com/paxos/ur/pull/42".to_string();
        let display = render_pr_hyperlink_display(&wf, 80);
        assert!(
            display.contains("paxos/ur#42"),
            "expected short PR format in output, got: {display:?}"
        );
    }

    #[test]
    fn pr_url_non_github_displays_raw_url() {
        let mut wf = make_workflow("done");
        wf.pr_url = "https://example.com/pr/99".to_string();
        let display = render_pr_hyperlink_display(&wf, 80);
        assert!(
            display.contains("https://example.com"),
            "expected raw URL in output, got: {display:?}"
        );
    }
}
