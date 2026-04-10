use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use ur_rpc::lifecycle;
use ur_rpc::proto::ticket::WorkflowInfo;

use crate::cmd::{Cmd, FetchCmd};
use crate::components::{MiniProgressBar, ThemedTable};
use crate::context::TuiContext;
use crate::input::{FooterCommand, InputHandler, InputResult};
use crate::model::{FLOW_PAGE_SIZE, FlowListData, FlowListModel, LoadState, Model};
use crate::msg::{FlowOpMsg, GotoTarget, Msg, NavMsg, OverlayMsg};
use crate::navigation::PageId;

/// Column index of the progress count label in the table.
const PROGRESS_COUNT_COL: usize = 3;
/// Column index of the progress bar in the table.
const PROGRESS_BAR_COL: usize = 4;

/// Render the flows list page into the given content area.
pub fn render_flows_list(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    match &model.flow_list.data {
        LoadState::NotLoaded | LoadState::Loading => {
            render_message(area, buf, ctx, "Loading...");
        }
        LoadState::Error(msg) => {
            render_message(area, buf, ctx, &format!("Error: {msg}"));
        }
        LoadState::Loaded(data) => {
            render_loaded_flows(area, buf, ctx, &model.flow_list, data);
        }
    }
}

/// Render a simple message in the content area.
fn render_message(area: Rect, buf: &mut Buffer, ctx: &TuiContext, msg: &str) {
    let style = Style::default()
        .fg(ctx.theme.base_content)
        .bg(ctx.theme.base_100);
    Paragraph::new(Line::raw(msg))
        .style(style)
        .render(area, buf);
}

/// Render the flows table when data is loaded.
fn render_loaded_flows(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    flow_model: &FlowListModel,
    data: &FlowListData,
) {
    let now = Utc::now();
    let page = flow_model.current_page;
    let total_pages = flow_total_pages(data.total_count);
    let page_workflows = page_slice(&data.workflows, page);

    let rows: Vec<Vec<String>> = page_workflows
        .iter()
        .map(|wf| workflow_to_row(wf, now))
        .collect();

    let selected = if rows.is_empty() {
        None
    } else {
        Some(flow_model.selected_row)
    };

    let page_info = format!(
        "Page {}/{} ({} total)",
        page + 1,
        total_pages,
        data.total_count
    );

    let widths = table_widths();

    let table = ThemedTable {
        headers: vec![
            "Ticket ID",
            "Status",
            "Stalled",
            "Progress",
            "",
            "Stage Time",
            "Total Time",
            "PR URL",
        ],
        rows,
        selected,
        widths: widths.clone(),
        page_info: Some(page_info),
    };

    let scroll_offset = table.render(area, buf, ctx);

    render_progress_bars(
        flow_model,
        &page_workflows,
        area,
        buf,
        ctx,
        &widths,
        scroll_offset,
    );
}

/// Build the column widths for the flows table (matching v1 layout).
fn table_widths() -> Vec<Constraint> {
    vec![
        Constraint::Length(12), // Ticket ID
        Constraint::Length(14), // Status
        Constraint::Length(8),  // Stalled
        Constraint::Length(8),  // Progress count
        Constraint::Length(11), // Progress bar
        Constraint::Length(12), // Stage Time
        Constraint::Length(12), // Total Time
        Constraint::Length(45), // PR URL
    ]
}

/// Convert a WorkflowInfo into a row of display strings.
fn workflow_to_row(wf: &WorkflowInfo, now: DateTime<Utc>) -> Vec<String> {
    let stalled_text = if wf.stalled {
        "✗".to_string()
    } else {
        String::new()
    };

    vec![
        wf.ticket_id.clone(),
        wf.status.clone(),
        stalled_text,
        String::new(), // placeholder for progress count
        String::new(), // placeholder for progress bar
        compute_stage_time(wf, now),
        compute_total_time(wf, now),
        wf.pr_url.clone(),
    ]
}

/// Compute progress (completed, total) for a workflow's child tickets.
fn workflow_progress(wf: &WorkflowInfo) -> (u32, u32) {
    let total = wf.ticket_children_open + wf.ticket_children_closed;
    if total > 0 {
        (wf.ticket_children_closed as u32, total as u32)
    } else if is_terminal_status(&wf.status) {
        (1, 1)
    } else {
        (0, 1)
    }
}

/// Check if a workflow status is terminal (times should be frozen).
fn is_terminal_status(status: &str) -> bool {
    status == lifecycle::DONE || status == lifecycle::CANCELLED
}

/// Parse an RFC3339 timestamp string into a DateTime<Utc>.
fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Format a chrono Duration as HH:MM:SS.
fn format_duration_hhmmss(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds().max(0);
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Compute stage time: duration from the last history event to now (or frozen
/// if terminal).
fn compute_stage_time(wf: &WorkflowInfo, now: DateTime<Utc>) -> String {
    let last = wf
        .history
        .iter()
        .filter_map(|evt| parse_rfc3339(&evt.created_at))
        .next_back();
    let Some(last) = last else {
        return "-".to_string();
    };
    let end = if is_terminal_status(&wf.status) {
        last
    } else {
        now
    };
    format_duration_hhmmss(end - last)
}

/// Compute total time: duration from workflow creation to now (or frozen if
/// terminal).
fn compute_total_time(wf: &WorkflowInfo, now: DateTime<Utc>) -> String {
    let first = parse_rfc3339(&wf.created_at);
    let Some(first) = first else {
        return "-".to_string();
    };
    let last = wf
        .history
        .iter()
        .filter_map(|evt| parse_rfc3339(&evt.created_at))
        .next_back();
    let end = if is_terminal_status(&wf.status) {
        last.unwrap_or(first)
    } else {
        now
    };
    format_duration_hhmmss(end - first)
}

/// Calculate total number of pages for flow list.
fn flow_total_pages(total_count: i32) -> usize {
    if total_count <= 0 {
        1
    } else {
        (total_count as usize).div_ceil(FLOW_PAGE_SIZE)
    }
}

/// Get the workflows for the current page (client-side slice since server
/// returns full list).
fn page_slice(workflows: &[WorkflowInfo], page: usize) -> Vec<&WorkflowInfo> {
    let start = page * FLOW_PAGE_SIZE;
    let end = (start + FLOW_PAGE_SIZE).min(workflows.len());
    if start >= workflows.len() {
        vec![]
    } else {
        workflows[start..end].iter().collect()
    }
}

/// Render mini progress bars and count labels over the placeholder columns.
fn render_progress_bars(
    flow_model: &FlowListModel,
    page_workflows: &[&WorkflowInfo],
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    widths: &[Constraint],
    scroll_offset: usize,
) {
    let inner = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let col_areas = Layout::horizontal(widths.to_vec()).split(inner);
    let bar_area = col_areas.get(PROGRESS_BAR_COL);
    let count_area = col_areas.get(PROGRESS_COUNT_COL);

    if bar_area.is_none() && count_area.is_none() {
        return;
    }

    let data_start_y = inner.y + 1;

    for (i, wf) in page_workflows.iter().enumerate().skip(scroll_offset) {
        let row_y = data_start_y + (i - scroll_offset) as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let (completed, total) = workflow_progress(wf);
        let is_selected = i == flow_model.selected_row;
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
        let is_stalled = wf.stalled;

        if let Some(ba) = bar_area {
            let cell = Rect {
                x: ba.x,
                y: row_y,
                width: ba.width,
                height: 1,
            };
            if is_stalled {
                bar.render_bar_colored(cell, buf, ctx.theme.error, row_bg);
            } else {
                bar.render_bar(cell, buf, &ctx.theme, row_bg);
            }
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

/// Get the currently selected workflow from the model, if any.
fn selected_workflow(model: &Model) -> Option<&WorkflowInfo> {
    let data = model.flow_list.data.data()?;
    let page_wfs = page_slice(&data.workflows, model.flow_list.current_page);
    page_wfs.get(model.flow_list.selected_row).copied()
}

/// Apply loaded flow data to the flow list model.
pub fn apply_flows_data(model: &mut Model, data: &FlowListData) {
    let count = page_count_for(data, model.flow_list.current_page);
    if count > 0 && model.flow_list.selected_row >= count {
        model.flow_list.selected_row = count - 1;
    }
}

/// Count items on a given page.
fn page_count_for(data: &FlowListData, page: usize) -> usize {
    let total = data.workflows.len();
    let start = page * FLOW_PAGE_SIZE;
    if start >= total {
        return 0;
    }
    (start + FLOW_PAGE_SIZE).min(total) - start
}

/// Build a fetch command for the flow list.
pub fn build_flow_list_fetch_cmd(model: &Model) -> Cmd {
    let offset = (model.flow_list.current_page * FLOW_PAGE_SIZE) as i32;
    Cmd::Fetch(FetchCmd::Flows {
        page_size: Some(FLOW_PAGE_SIZE as i32),
        offset: Some(offset),
    })
}

/// Handle flow list navigation messages.
pub fn handle_flows_nav(mut model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    match nav_msg {
        NavMsg::FlowsNavigate { delta } => {
            flows_navigate(&mut model, delta);
            (model, vec![])
        }
        NavMsg::FlowsPageRight => {
            flows_page_right(&mut model);
            (model, vec![])
        }
        NavMsg::FlowsPageLeft => {
            flows_page_left(&mut model);
            (model, vec![])
        }
        NavMsg::FlowsRefresh => {
            model.flow_list.data = LoadState::Loading;
            let cmd = build_flow_list_fetch_cmd(&model);
            (model, vec![cmd])
        }
        NavMsg::FlowsSelect => handle_select(model),
        NavMsg::FlowsCancel => handle_cancel(model),
        NavMsg::FlowsApprove => handle_approve(model),
        NavMsg::FlowsRedrive => handle_redrive(model),
        NavMsg::FlowsGoto => handle_goto(model),
        _ => (model, vec![]),
    }
}

/// Navigate within the flows table by delta.
fn flows_navigate(model: &mut Model, delta: i32) {
    let count = current_page_count(model);
    if count == 0 {
        return;
    }
    let new = (model.flow_list.selected_row as i32 + delta)
        .max(0)
        .min(count as i32 - 1) as usize;
    model.flow_list.selected_row = new;
}

/// Move to the next page.
fn flows_page_right(model: &mut Model) {
    let total_count = model
        .flow_list
        .data
        .data()
        .map(|d| d.total_count)
        .unwrap_or(0);
    let tp = flow_total_pages(total_count);
    if model.flow_list.current_page + 1 < tp {
        model.flow_list.current_page += 1;
        model.flow_list.selected_row = 0;
    }
}

/// Move to the previous page.
fn flows_page_left(model: &mut Model) {
    if model.flow_list.current_page > 0 {
        model.flow_list.current_page -= 1;
        model.flow_list.selected_row = 0;
    }
}

/// Get the number of flows on the current page.
fn current_page_count(model: &Model) -> usize {
    model
        .flow_list
        .data
        .data()
        .map(|d| page_count_for(d, model.flow_list.current_page))
        .unwrap_or(0)
}

/// Push FlowDetail for the selected flow.
fn handle_select(mut model: Model) -> (Model, Vec<Cmd>) {
    if let Some(wf) = selected_workflow(&model) {
        let ticket_id = wf.ticket_id.clone();
        let page = PageId::FlowDetail { ticket_id };
        let mut nav = std::mem::replace(
            &mut model.navigation_model,
            crate::navigation::NavigationModel::initial(),
        );
        let cmds = nav.push(page, &mut model);
        model.navigation_model = nav;
        (model, cmds)
    } else {
        (model, vec![])
    }
}

/// Cancel the selected flow's workflow.
fn handle_cancel(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(wf) = selected_workflow(&model) {
        let msg = Msg::FlowOp(FlowOpMsg::Cancel {
            ticket_id: wf.ticket_id.clone(),
        });
        crate::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Approve the selected flow's workflow.
fn handle_approve(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(wf) = selected_workflow(&model) {
        let msg = Msg::FlowOp(FlowOpMsg::Approve {
            ticket_id: wf.ticket_id.clone(),
        });
        crate::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Redrive the selected flow's workflow.
fn handle_redrive(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(wf) = selected_workflow(&model) {
        let msg = Msg::TicketOp(crate::msg::TicketOpMsg::Redrive {
            ticket_id: wf.ticket_id.clone(),
        });
        crate::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Open the goto menu for the selected flow.
fn handle_goto(model: Model) -> (Model, Vec<Cmd>) {
    if let Some(wf) = selected_workflow(&model) {
        let targets = build_flow_goto_targets(&wf.ticket_id);
        let msg = Msg::Overlay(OverlayMsg::OpenGotoMenu { targets });
        crate::update::update(model, msg)
    } else {
        (model, vec![])
    }
}

/// Build goto targets for a flow with the given ticket ID.
fn build_flow_goto_targets(ticket_id: &str) -> Vec<GotoTarget> {
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

/// Clamp the selected row after data changes.
pub fn clamp_selection(model: &mut Model) {
    let count = current_page_count(model);
    if count == 0 {
        model.flow_list.selected_row = 0;
    } else if model.flow_list.selected_row >= count {
        model.flow_list.selected_row = count.saturating_sub(1);
    }
}

/// Input handler for the flows list page.
pub struct FlowListHandler;

impl InputHandler for FlowListHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        if key.modifiers == KeyModifiers::NONE
            && let Some(msg) = handle_table_key(key.code)
        {
            return InputResult::Capture(msg);
        }

        if let Some(msg) = handle_operation_key(key) {
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
                description: "Move to Verify".to_string(),
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
            FooterCommand {
                key_label: "r".to_string(),
                description: "Refresh".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "Space".to_string(),
                description: "Details".to_string(),
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
        "flow_list"
    }
}

/// Handle navigation keys (no modifiers).
fn handle_table_key(code: KeyCode) -> Option<Msg> {
    match code {
        KeyCode::Char('k') | KeyCode::Up => Some(Msg::Nav(NavMsg::FlowsNavigate { delta: -1 })),
        KeyCode::Char('j') | KeyCode::Down => Some(Msg::Nav(NavMsg::FlowsNavigate { delta: 1 })),
        KeyCode::Char('h') | KeyCode::Left => Some(Msg::Nav(NavMsg::FlowsPageLeft)),
        KeyCode::Char('l') | KeyCode::Right => Some(Msg::Nav(NavMsg::FlowsPageRight)),
        KeyCode::Char(' ') | KeyCode::Enter => Some(Msg::Nav(NavMsg::FlowsSelect)),
        KeyCode::Char('r') => Some(Msg::Nav(NavMsg::FlowsRefresh)),
        KeyCode::Char('g') => Some(Msg::Nav(NavMsg::FlowsGoto)),
        _ => None,
    }
}

/// Handle Shift+letter operation keys.
fn handle_operation_key(key: KeyEvent) -> Option<Msg> {
    if !key.modifiers.contains(KeyModifiers::SHIFT) {
        return None;
    }
    match key.code {
        KeyCode::Char('A') => Some(Msg::Nav(NavMsg::FlowsApprove)),
        KeyCode::Char('X') => Some(Msg::Nav(NavMsg::FlowsCancel)),
        KeyCode::Char('V') => Some(Msg::Nav(NavMsg::FlowsRedrive)),
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

    fn make_workflow(ticket_id: &str, status: &str) -> WorkflowInfo {
        WorkflowInfo {
            id: format!("wf-{ticket_id}"),
            ticket_id: ticket_id.into(),
            status: status.into(),
            stalled: false,
            stall_reason: String::new(),
            implement_cycles: 1,
            worker_id: "w-1".into(),
            feedback_mode: "auto".into(),
            created_at: "2026-03-22T10:00:00+00:00".into(),
            pr_url: String::new(),
            history: vec![],
            ticket_children_open: 0,
            ticket_children_closed: 0,
        }
    }

    fn model_with_flows(workflows: Vec<WorkflowInfo>) -> Model {
        let total_count = workflows.len() as i32;
        let mut model = Model::initial();
        model.flow_list.data = LoadState::Loaded(FlowListData {
            workflows,
            total_count,
        });
        model
    }

    // ── workflow_to_row ─────────────────────────────────────────────

    #[test]
    fn workflow_to_row_basic() {
        let wf = make_workflow("ur-abc", "implementing");
        let now = Utc::now();
        let row = workflow_to_row(&wf, now);
        assert_eq!(row[0], "ur-abc");
        assert_eq!(row[1], "implementing");
    }

    #[test]
    fn workflow_to_row_stalled() {
        let mut wf = make_workflow("ur-abc", "implementing");
        wf.stalled = true;
        let now = Utc::now();
        let row = workflow_to_row(&wf, now);
        assert_eq!(row[2], "✗");
    }

    // ── workflow_progress ────────────────────────────────────────────

    #[test]
    fn progress_with_children() {
        let mut wf = make_workflow("ur-abc", "implementing");
        wf.ticket_children_open = 3;
        wf.ticket_children_closed = 7;
        let (completed, total) = workflow_progress(&wf);
        assert_eq!(completed, 7);
        assert_eq!(total, 10);
    }

    #[test]
    fn progress_terminal_no_children() {
        let wf = make_workflow("ur-abc", "done");
        let (completed, total) = workflow_progress(&wf);
        assert_eq!(completed, 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn progress_nonterminal_no_children() {
        let wf = make_workflow("ur-abc", "implementing");
        let (completed, total) = workflow_progress(&wf);
        assert_eq!(completed, 0);
        assert_eq!(total, 1);
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

    // ── format_duration_hhmmss ──────────────────────────────────────

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

    // ── time computation ────────────────────────────────────────────

    #[test]
    fn stage_time_no_history() {
        let wf = make_workflow("ur-abc", "implementing");
        assert_eq!(compute_stage_time(&wf, Utc::now()), "-");
    }

    #[test]
    fn stage_time_with_history() {
        let mut wf = make_workflow("ur-abc", "done");
        wf.history = vec![
            WorkflowHistoryEvent {
                event: "started".into(),
                created_at: "2026-03-22T10:00:00+00:00".into(),
            },
            WorkflowHistoryEvent {
                event: "done".into(),
                created_at: "2026-03-22T11:00:00+00:00".into(),
            },
        ];
        // Terminal: stage time = end - last = 0
        assert_eq!(compute_stage_time(&wf, Utc::now()), "00:00:00");
    }

    #[test]
    fn total_time_no_created_at() {
        let mut wf = make_workflow("ur-abc", "implementing");
        wf.created_at = String::new();
        assert_eq!(compute_total_time(&wf, Utc::now()), "-");
    }

    // ── flow_total_pages ────────────────────────────────────────────

    #[test]
    fn total_pages_empty() {
        assert_eq!(flow_total_pages(0), 1);
    }

    #[test]
    fn total_pages_exact() {
        assert_eq!(flow_total_pages(FLOW_PAGE_SIZE as i32), 1);
    }

    #[test]
    fn total_pages_partial() {
        assert_eq!(flow_total_pages(FLOW_PAGE_SIZE as i32 + 1), 2);
    }

    // ── navigation ──────────────────────────────────────────────────

    #[test]
    fn navigate_down() {
        let workflows: Vec<WorkflowInfo> = (0..3)
            .map(|i| make_workflow(&format!("ur-{i}"), "implementing"))
            .collect();
        let mut model = model_with_flows(workflows);

        flows_navigate(&mut model, 1);
        assert_eq!(model.flow_list.selected_row, 1);
    }

    #[test]
    fn navigate_up() {
        let workflows: Vec<WorkflowInfo> = (0..3)
            .map(|i| make_workflow(&format!("ur-{i}"), "implementing"))
            .collect();
        let mut model = model_with_flows(workflows);
        model.flow_list.selected_row = 2;

        flows_navigate(&mut model, -1);
        assert_eq!(model.flow_list.selected_row, 1);
    }

    #[test]
    fn navigate_empty_is_noop() {
        let mut model = model_with_flows(vec![]);
        flows_navigate(&mut model, 1);
        assert_eq!(model.flow_list.selected_row, 0);
    }

    #[test]
    fn page_right_advances() {
        let workflows: Vec<WorkflowInfo> = (0..45)
            .map(|i| make_workflow(&format!("ur-{i:02}"), "implementing"))
            .collect();
        let mut model = model_with_flows(workflows);

        flows_page_right(&mut model);
        assert_eq!(model.flow_list.current_page, 1);
        assert_eq!(model.flow_list.selected_row, 0);
    }

    #[test]
    fn page_left_goes_back() {
        let workflows: Vec<WorkflowInfo> = (0..45)
            .map(|i| make_workflow(&format!("ur-{i:02}"), "implementing"))
            .collect();
        let mut model = model_with_flows(workflows);
        model.flow_list.current_page = 1;

        flows_page_left(&mut model);
        assert_eq!(model.flow_list.current_page, 0);
    }

    #[test]
    fn page_left_at_zero_is_noop() {
        let mut model = model_with_flows(vec![]);
        flows_page_left(&mut model);
        assert_eq!(model.flow_list.current_page, 0);
    }

    // ── input handler ───────────────────────────────────────────────

    #[test]
    fn handler_j_captures_navigate_down() {
        let handler = FlowListHandler;
        match handler.handle_key(plain_key(KeyCode::Char('j'))) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowsNavigate { delta: 1 })) => {}
            other => panic!("expected navigate down, got {other:?}"),
        }
    }

    #[test]
    fn handler_k_captures_navigate_up() {
        let handler = FlowListHandler;
        match handler.handle_key(plain_key(KeyCode::Char('k'))) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowsNavigate { delta: -1 })) => {}
            other => panic!("expected navigate up, got {other:?}"),
        }
    }

    #[test]
    fn handler_space_captures_select() {
        let handler = FlowListHandler;
        match handler.handle_key(plain_key(KeyCode::Char(' '))) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowsSelect)) => {}
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn handler_enter_captures_select() {
        let handler = FlowListHandler;
        match handler.handle_key(plain_key(KeyCode::Enter)) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowsSelect)) => {}
            other => panic!("expected select, got {other:?}"),
        }
    }

    #[test]
    fn handler_shift_a_captures_approve() {
        let handler = FlowListHandler;
        let key = make_key(KeyCode::Char('A'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowsApprove)) => {}
            other => panic!("expected approve, got {other:?}"),
        }
    }

    #[test]
    fn handler_shift_x_captures_cancel() {
        let handler = FlowListHandler;
        let key = make_key(KeyCode::Char('X'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowsCancel)) => {}
            other => panic!("expected cancel, got {other:?}"),
        }
    }

    #[test]
    fn handler_shift_v_captures_redrive() {
        let handler = FlowListHandler;
        let key = make_key(KeyCode::Char('V'), KeyModifiers::SHIFT);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::FlowsRedrive)) => {}
            other => panic!("expected redrive, got {other:?}"),
        }
    }

    #[test]
    fn handler_bubbles_unrecognized() {
        let handler = FlowListHandler;
        assert!(matches!(
            handler.handle_key(plain_key(KeyCode::Char('z'))),
            InputResult::Bubble
        ));
    }

    #[test]
    fn handler_has_footer_commands() {
        let handler = FlowListHandler;
        let cmds = handler.footer_commands();
        assert!(!cmds.is_empty());
        assert!(cmds.iter().any(|c| c.description == "Approve"));
        assert!(cmds.iter().any(|c| c.description == "Cancel"));
        assert!(cmds.iter().any(|c| c.description == "Move to Verify"));
        assert!(cmds.iter().any(|c| c.description == "Goto"));
        assert!(cmds.iter().any(|c| c.description == "Refresh"));
        assert!(
            cmds.iter()
                .any(|c| c.key_label == "Space" && c.description == "Details")
        );
    }

    #[test]
    fn handler_name() {
        let handler = FlowListHandler;
        assert_eq!(handler.name(), "flow_list");
    }

    // ── handle_flows_nav integration ────────────────────────────────

    #[test]
    fn handle_nav_navigate() {
        let workflows: Vec<WorkflowInfo> = (0..3)
            .map(|i| make_workflow(&format!("ur-{i}"), "implementing"))
            .collect();
        let model = model_with_flows(workflows);

        let (new_model, cmds) = handle_flows_nav(model, NavMsg::FlowsNavigate { delta: 1 });
        assert_eq!(new_model.flow_list.selected_row, 1);
        assert!(cmds.is_empty());
    }

    #[test]
    fn handle_nav_refresh() {
        let workflows = vec![make_workflow("ur-abc", "implementing")];
        let model = model_with_flows(workflows);

        let (new_model, cmds) = handle_flows_nav(model, NavMsg::FlowsRefresh);
        assert!(new_model.flow_list.data.is_loading());
        assert!(!cmds.is_empty());
    }

    #[test]
    fn handle_nav_select_pushes_detail() {
        let workflows = vec![make_workflow("ur-abc", "implementing")];
        let model = model_with_flows(workflows);

        let (new_model, _cmds) = handle_flows_nav(model, NavMsg::FlowsSelect);
        assert_eq!(
            new_model.navigation_model.current_page(),
            &PageId::FlowDetail {
                ticket_id: "ur-abc".to_string()
            }
        );
    }

    #[test]
    fn handle_nav_select_empty_is_noop() {
        let model = model_with_flows(vec![]);
        let (_, cmds) = handle_flows_nav(model, NavMsg::FlowsSelect);
        assert!(cmds.is_empty());
    }

    // ── clamp_selection ─────────────────────────────────────────────

    #[test]
    fn clamp_selection_on_empty() {
        let mut model = model_with_flows(vec![]);
        model.flow_list.selected_row = 5;
        clamp_selection(&mut model);
        assert_eq!(model.flow_list.selected_row, 0);
    }

    #[test]
    fn clamp_selection_within_bounds() {
        let workflows = vec![
            make_workflow("ur-a", "implementing"),
            make_workflow("ur-b", "implementing"),
        ];
        let mut model = model_with_flows(workflows);
        model.flow_list.selected_row = 1;
        clamp_selection(&mut model);
        assert_eq!(model.flow_list.selected_row, 1);
    }

    // ── goto targets ────────────────────────────────────────────────

    #[test]
    fn goto_targets_include_standard_options() {
        let targets = build_flow_goto_targets("ur-abc");
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|t| t.label == "Ticket Details"));
        assert!(targets.iter().any(|t| t.label == "Worker"));
    }
}
