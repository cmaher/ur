use std::collections::HashMap;
use std::time::Instant;

use chrono::{DateTime, Utc};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use ur_rpc::lifecycle;
use ur_rpc::proto::ticket::WorkflowInfo;

use crate::context::TuiContext;
use crate::data::{ActionResult, DataPayload};
use crate::keymap::{Action, Keymap};
use crate::page::{Banner, BannerVariant, FooterCommand, Page, PageResult, StatusMessage, TabId};
use crate::pages::flow_detail::{detail_footer_commands, render_flow_detail};
use crate::widgets::{MiniProgressBar, ThemedTable};

const PAGE_SIZE: usize = 20;

/// Parsed timestamps from workflow history, computed once on data receipt.
struct ParsedTimestamps {
    /// Timestamp of the first history event (workflow start).
    first: Option<DateTime<Utc>>,
    /// Timestamp of the last history event (most recent transition).
    last: Option<DateTime<Utc>>,
}

/// A workflow entry with its pre-parsed timestamps.
struct FlowEntry {
    workflow: WorkflowInfo,
    timestamps: ParsedTimestamps,
}

/// State for the Flows tab, showing workflows in a server-paginated table.
///
/// Only one page of data is held at a time. Page navigation triggers a new
/// gRPC fetch with the appropriate offset.
pub struct FlowsPage {
    /// Map of ticket_id -> FlowEntry for the current page only.
    entry_map: HashMap<String, FlowEntry>,
    /// Sorted display list of ticket IDs for the current page.
    display_ids: Vec<String>,
    selected: usize,
    page: usize,
    /// Server-reported total number of workflows (across all pages).
    total_count: i32,
    loaded: bool,
    error: Option<String>,
    refreshing: bool,
    /// In-progress status message shown below the tab header.
    active_status: Option<StatusMessage>,
    /// Active notification banner (success/error from async actions).
    active_banner: Option<Banner>,
    /// When Some, the detail sub-page is shown for this workflow.
    detail_workflow: Option<WorkflowInfo>,
    /// When true, a page navigation is pending and needs a server fetch.
    pending_fetch: bool,
}

impl FlowsPage {
    pub fn new() -> Self {
        Self {
            entry_map: HashMap::new(),
            display_ids: Vec::new(),
            selected: 0,
            page: 0,
            total_count: 0,
            loaded: false,
            error: None,
            refreshing: false,
            active_status: None,
            active_banner: None,
            detail_workflow: None,
            pending_fetch: false,
        }
    }

    /// Total number of pages based on server-reported total_count.
    fn total_pages(&self) -> usize {
        if self.total_count <= 0 {
            1
        } else {
            (self.total_count as usize).div_ceil(PAGE_SIZE)
        }
    }

    /// Returns the page size used for server pagination requests.
    pub fn page_size(&self) -> i32 {
        PAGE_SIZE as i32
    }

    /// Returns the current offset for server pagination requests.
    pub fn page_offset(&self) -> i32 {
        (self.page * PAGE_SIZE) as i32
    }

    fn page_entries(&self) -> Vec<&FlowEntry> {
        self.display_ids
            .iter()
            .filter_map(|id| self.entry_map.get(id))
            .collect()
    }

    fn page_row_count(&self) -> usize {
        self.display_ids.len()
    }

    fn clamp_selection(&mut self) {
        let count = self.page_row_count();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    /// Rebuild the display_ids list from the entry_map.
    fn rebuild_display_ids(&mut self) {
        self.display_ids = self.entry_map.keys().cloned().collect();
        self.display_ids.sort();
        self.clamp_selection();
    }

    /// Returns the ticket ID of the currently selected workflow, if any.
    pub fn selected_ticket_id(&self) -> Option<String> {
        self.display_ids.get(self.selected).cloned()
    }

    /// Handle an async action result by showing a success or error banner.
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

    /// Load a page of workflows from the server response, replacing current data.
    fn load_page(&mut self, workflows: &[WorkflowInfo], total_count: i32) {
        self.entry_map.clear();
        for wf in workflows {
            let timestamps = parse_timestamps(wf);
            let ticket_id = wf.ticket_id.clone();
            self.entry_map.insert(
                ticket_id,
                FlowEntry {
                    workflow: wf.clone(),
                    timestamps,
                },
            );
        }
        self.total_count = total_count;
        // Clamp page if offset is past the end (e.g. items were deleted).
        self.clamp_page_to_valid();
        self.rebuild_display_ids();
    }

    /// Clamp the current page to the last valid page if offset >= total_count.
    fn clamp_page_to_valid(&mut self) {
        let max_page = if self.total_count <= 0 {
            0
        } else {
            ((self.total_count as usize).saturating_sub(1)) / PAGE_SIZE
        };
        if self.page > max_page {
            self.page = max_page;
            self.selected = 0;
            // Need another fetch at the clamped offset.
            self.pending_fetch = true;
        }
    }

    /// Handle actions when in detail view mode.
    fn handle_detail_action(&mut self, action: Action) -> PageResult {
        match action {
            Action::Back => {
                self.detail_workflow = None;
                PageResult::Consumed
            }
            Action::Quit => PageResult::Quit,
            _ => PageResult::Consumed,
        }
    }

    /// Handle actions when in list view mode.
    fn handle_list_action(&mut self, action: Action) -> PageResult {
        match action {
            Action::NavigateUp => {
                if self.selected > 0 {
                    self.selected -= 1;
                }
                PageResult::Consumed
            }
            Action::NavigateDown => {
                let count = self.page_row_count();
                if count > 0 && self.selected < count - 1 {
                    self.selected += 1;
                }
                PageResult::Consumed
            }
            Action::PageLeft => {
                if self.page > 0 {
                    self.page -= 1;
                    self.selected = 0;
                    self.pending_fetch = true;
                }
                PageResult::Consumed
            }
            Action::PageRight => {
                if self.page + 1 < self.total_pages() {
                    self.page += 1;
                    self.selected = 0;
                    self.pending_fetch = true;
                }
                PageResult::Consumed
            }
            Action::Select => {
                if let Some(ticket_id) = self.selected_ticket_id()
                    && let Some(entry) = self.entry_map.get(&ticket_id)
                {
                    self.detail_workflow = Some(entry.workflow.clone());
                }
                PageResult::Consumed
            }
            Action::Refresh => {
                self.refreshing = true;
                self.active_status = Some(StatusMessage {
                    text: "Refreshing flows...".to_string(),
                    dismissable: true,
                });
                PageResult::Consumed
            }
            Action::CancelFlow | Action::CloseTicket => {
                // Handled at the app level in cancel_selected_flow().
                PageResult::Ignored
            }
            Action::Quit => PageResult::Quit,
            _ => PageResult::Ignored,
        }
    }
}

/// Parse history timestamps from a WorkflowInfo, extracting first and last.
fn parse_timestamps(wf: &WorkflowInfo) -> ParsedTimestamps {
    let parsed: Vec<DateTime<Utc>> = wf
        .history
        .iter()
        .filter_map(|evt| DateTime::parse_from_rfc3339(&evt.created_at).ok())
        .map(|dt| dt.with_timezone(&Utc))
        .collect();

    ParsedTimestamps {
        first: parsed.first().copied(),
        last: parsed.last().copied(),
    }
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

/// Compute stage time string for a workflow entry.
fn compute_stage_time(entry: &FlowEntry, now: DateTime<Utc>) -> String {
    let Some(last) = entry.timestamps.last else {
        return "-".to_string();
    };
    let end = if is_terminal_status(&entry.workflow.status) {
        last
    } else {
        now
    };
    format_duration_hhmmss(end - last)
}

/// Compute total time string for a workflow entry.
fn compute_total_time(entry: &FlowEntry, now: DateTime<Utc>) -> String {
    let Some(first) = entry.timestamps.first else {
        return "-".to_string();
    };
    let end = if is_terminal_status(&entry.workflow.status) {
        entry.timestamps.last.unwrap_or(first)
    } else {
        now
    };
    format_duration_hhmmss(end - first)
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

/// Convert a FlowEntry into a row of display strings.
fn entry_to_row(entry: &FlowEntry, now: DateTime<Utc>) -> Vec<String> {
    let wf = &entry.workflow;
    let stalled_text = if wf.stalled {
        "X".to_string()
    } else {
        String::new()
    };

    vec![
        wf.ticket_id.clone(),
        wf.status.clone(),
        String::new(), // placeholder for progress count
        String::new(), // placeholder for progress bar
        compute_stage_time(entry, now),
        compute_total_time(entry, now),
        wf.pr_url.clone(),
        stalled_text,
    ]
}

/// The column index of the progress count label in the table.
const PROGRESS_COUNT_COL: usize = 2;
/// The column index of the progress bar in the table.
const PROGRESS_BAR_COL: usize = 3;

/// Render mini progress bars and count labels over the placeholder columns.
fn render_progress_bars(
    page: &FlowsPage,
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
    let bar_area = col_areas.get(PROGRESS_BAR_COL);
    let count_area = col_areas.get(PROGRESS_COUNT_COL);

    if bar_area.is_none() && count_area.is_none() {
        return;
    }

    let data_start_y = inner.y + 1;
    let entries = page.page_entries();

    for (i, entry) in entries.into_iter().enumerate() {
        let row_y = data_start_y + i as u16;
        if row_y >= inner.y + inner.height {
            break;
        }

        let (completed, total) = workflow_progress(&entry.workflow);
        let is_selected = i == page.selected;
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

impl Page for FlowsPage {
    fn tab_id(&self) -> TabId {
        TabId::Flows
    }

    fn title(&self) -> &str {
        "Flows"
    }

    fn shortcut_char(&self) -> char {
        'f'
    }

    fn handle_action(&mut self, action: Action) -> PageResult {
        if self.detail_workflow.is_some() {
            return self.handle_detail_action(action);
        }
        self.handle_list_action(action)
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        if !self.loaded {
            let msg = Line::raw("Loading...");
            let paragraph = Paragraph::new(msg).style(Style::default().fg(ctx.theme.base_content));
            paragraph.render(area, buf);
            return;
        }

        if let Some(ref err) = self.error {
            let msg = Line::raw(format!("Error: {err}"));
            let paragraph = Paragraph::new(msg).style(Style::default().fg(ctx.theme.error));
            paragraph.render(area, buf);
            return;
        }

        if let Some(ref wf) = self.detail_workflow {
            render_flow_detail(wf, area, buf, ctx);
            return;
        }

        let now = Utc::now();
        let rows: Vec<Vec<String>> = self
            .page_entries()
            .into_iter()
            .map(|entry| entry_to_row(entry, now))
            .collect();

        let selected = if rows.is_empty() {
            None
        } else {
            Some(self.selected)
        };

        let page_info = format!("Page {}/{}", self.page + 1, self.total_pages());

        let widths = vec![
            Constraint::Length(12), // Ticket ID
            Constraint::Length(14), // Status
            Constraint::Length(8),  // Progress count
            Constraint::Length(10), // Progress bar
            Constraint::Length(12), // Stage Time
            Constraint::Length(12), // Total Time
            Constraint::Length(45), // PR URL
            Constraint::Length(8),  // Stalled
        ];

        let table = ThemedTable {
            headers: vec![
                "Ticket ID",
                "Status",
                "Progress",
                "",
                "Stage Time",
                "Total Time",
                "PR URL",
                "Stalled",
            ],
            rows,
            selected,
            widths: widths.clone(),
            page_info: Some(page_info),
        };

        table.render(area, buf, ctx);

        render_progress_bars(self, area, buf, ctx, &widths);
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        if self.detail_workflow.is_some() {
            return detail_footer_commands(keymap);
        }
        vec![
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
                key_label: keymap.label_for(&Action::Select),
                description: "Select".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: keymap.label_for(&Action::CloseTicket),
                description: "Cancel".to_string(),
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
            DataPayload::Flows(Ok((workflows, total_count))) => {
                self.loaded = true;
                self.refreshing = false;
                self.pending_fetch = false;
                self.active_status = None;
                self.error = None;
                self.load_page(workflows, *total_count);
            }
            DataPayload::Flows(Err(msg)) => {
                self.loaded = true;
                self.refreshing = false;
                self.pending_fetch = false;
                self.active_status = None;
                self.error = Some(msg.clone());
                self.entry_map.clear();
                self.display_ids.clear();
            }
            _ => {}
        }
    }

    fn needs_data(&self) -> bool {
        !self.loaded || self.refreshing || self.pending_fetch
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

    fn set_status(&mut self, text: String) {
        self.active_status = Some(StatusMessage {
            text,
            dismissable: false,
        });
    }

    fn clear_status(&mut self) {
        self.active_status = None;
    }

    fn mark_stale(&mut self) {
        self.loaded = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ur_rpc::proto::ticket::WorkflowHistoryEvent;

    fn make_workflow(id: &str, ticket_id: &str, stalled: bool) -> WorkflowInfo {
        WorkflowInfo {
            id: id.into(),
            ticket_id: ticket_id.into(),
            status: "implementing".into(),
            stalled,
            stall_reason: if stalled {
                "test stall".into()
            } else {
                String::new()
            },
            implement_cycles: 2,
            worker_id: String::new(),
            feedback_mode: String::new(),
            created_at: String::new(),
            pr_url: "https://github.com/org/repo/pull/1".into(),
            history: vec![],
            ticket_children_open: 0,
            ticket_children_closed: 0,
        }
    }

    fn make_workflow_with_history(
        ticket_id: &str,
        status: &str,
        history: Vec<WorkflowHistoryEvent>,
    ) -> WorkflowInfo {
        WorkflowInfo {
            id: "wf-1".into(),
            ticket_id: ticket_id.into(),
            status: status.into(),
            stalled: false,
            stall_reason: String::new(),
            implement_cycles: 1,
            worker_id: String::new(),
            feedback_mode: String::new(),
            created_at: String::new(),
            pr_url: String::new(),
            history,
            ticket_children_open: 0,
            ticket_children_closed: 0,
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
    fn format_duration_large() {
        let d = chrono::Duration::seconds(86400); // 24 hours
        assert_eq!(format_duration_hhmmss(d), "24:00:00");
    }

    #[test]
    fn format_duration_negative_clamps_to_zero() {
        let d = chrono::Duration::seconds(-10);
        assert_eq!(format_duration_hhmmss(d), "00:00:00");
    }

    #[test]
    fn terminal_status_done() {
        assert!(is_terminal_status("done"));
    }

    #[test]
    fn terminal_status_cancelled() {
        assert!(is_terminal_status("cancelled"));
    }

    #[test]
    fn non_terminal_status() {
        assert!(!is_terminal_status("implementing"));
        assert!(!is_terminal_status("open"));
    }

    #[test]
    fn empty_history_shows_dash() {
        let wf = make_workflow_with_history("ur-abc", "implementing", vec![]);
        let entry = FlowEntry {
            timestamps: parse_timestamps(&wf),
            workflow: wf,
        };
        let now = Utc::now();
        assert_eq!(compute_stage_time(&entry, now), "-");
        assert_eq!(compute_total_time(&entry, now), "-");
    }

    #[test]
    fn terminal_workflow_freezes_times() {
        let t1 = "2026-03-22T10:00:00+00:00";
        let t2 = "2026-03-22T11:30:00+00:00";
        let history = vec![
            WorkflowHistoryEvent {
                event: "started".into(),
                created_at: t1.into(),
            },
            WorkflowHistoryEvent {
                event: "completed".into(),
                created_at: t2.into(),
            },
        ];
        let wf = make_workflow_with_history("ur-abc", "done", history);
        let entry = FlowEntry {
            timestamps: parse_timestamps(&wf),
            workflow: wf,
        };
        // Use a time far in the future - should not affect frozen times
        let future = DateTime::parse_from_rfc3339("2030-01-01T00:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);

        // Stage time: last - last = 0 (frozen at last event)
        assert_eq!(compute_stage_time(&entry, future), "00:00:00");
        // Total time: last - first = 1h30m
        assert_eq!(compute_total_time(&entry, future), "01:30:00");
    }

    #[test]
    fn new_page_needs_data() {
        let page = FlowsPage::new();
        assert!(page.needs_data());
        assert!(!page.loaded);
    }

    #[test]
    fn on_data_loads_workflows() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok((wfs.clone(), 1))));
        assert!(page.loaded);
        assert!(!page.needs_data());
        assert_eq!(page.entry_map.len(), 1);
        assert!(page.error.is_none());
        assert_eq!(page.total_count, 1);
    }

    #[test]
    fn on_data_handles_error() {
        let mut page = FlowsPage::new();
        page.on_data(&DataPayload::Flows(Err("connection refused".into())));
        assert!(page.loaded);
        assert!(page.error.is_some());
        assert!(page.entry_map.is_empty());
    }

    #[test]
    fn on_data_ignores_tickets_payload() {
        let mut page = FlowsPage::new();
        page.on_data(&DataPayload::Tickets(Ok((vec![], 0))));
        assert!(!page.loaded);
    }

    #[test]
    fn navigate_up_down() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..3)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 3))));

        assert_eq!(page.selected, 0);
        assert_eq!(
            page.handle_action(Action::NavigateDown),
            PageResult::Consumed
        );
        assert_eq!(page.selected, 1);
        assert_eq!(
            page.handle_action(Action::NavigateDown),
            PageResult::Consumed
        );
        assert_eq!(page.selected, 2);
        // At bottom, stays at 2
        assert_eq!(
            page.handle_action(Action::NavigateDown),
            PageResult::Consumed
        );
        assert_eq!(page.selected, 2);

        assert_eq!(page.handle_action(Action::NavigateUp), PageResult::Consumed);
        assert_eq!(page.selected, 1);
        assert_eq!(page.handle_action(Action::NavigateUp), PageResult::Consumed);
        assert_eq!(page.selected, 0);
        // At top, stays at 0
        assert_eq!(page.handle_action(Action::NavigateUp), PageResult::Consumed);
        assert_eq!(page.selected, 0);
    }

    #[test]
    fn page_right_triggers_fetch() {
        let mut page = FlowsPage::new();
        // Simulate server returning first page of 20 items, total 45
        let wfs: Vec<WorkflowInfo> = (0..20)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 45))));

        assert_eq!(page.total_pages(), 3);
        assert_eq!(page.page, 0);
        assert!(!page.needs_data());

        // PageRight should trigger a pending fetch
        assert_eq!(page.handle_action(Action::PageRight), PageResult::Consumed);
        assert_eq!(page.page, 1);
        assert_eq!(page.selected, 0);
        assert!(page.pending_fetch);
        assert!(page.needs_data());
        assert_eq!(page.page_offset(), 20);
    }

    #[test]
    fn page_left_triggers_fetch() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..20)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 45))));

        // Move to page 1
        page.handle_action(Action::PageRight);
        // Simulate server response for page 1
        let wfs2: Vec<WorkflowInfo> = (20..40)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs2, 45))));
        assert!(!page.needs_data());

        // PageLeft should trigger a pending fetch
        assert_eq!(page.handle_action(Action::PageLeft), PageResult::Consumed);
        assert_eq!(page.page, 0);
        assert!(page.pending_fetch);
        assert!(page.needs_data());
        assert_eq!(page.page_offset(), 0);
    }

    #[test]
    fn cannot_page_past_last() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..5)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 5))));

        // Only 1 page total, PageRight should not change anything
        assert_eq!(page.total_pages(), 1);
        assert_eq!(page.handle_action(Action::PageRight), PageResult::Consumed);
        assert_eq!(page.page, 0);
        assert!(!page.pending_fetch);
    }

    #[test]
    fn cannot_page_before_first() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..5)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 25))));

        // Already on page 0, PageLeft should not change anything
        assert_eq!(page.handle_action(Action::PageLeft), PageResult::Consumed);
        assert_eq!(page.page, 0);
        assert!(!page.pending_fetch);
    }

    #[test]
    fn total_pages_uses_server_total_count() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..10)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i}"), false))
            .collect();
        // Server says 45 total, even though we only got 10 on this page
        page.on_data(&DataPayload::Flows(Ok((wfs, 45))));
        assert_eq!(page.total_pages(), 3);
    }

    #[test]
    fn page_offset_and_size() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..20)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 60))));

        assert_eq!(page.page_size(), 20);
        assert_eq!(page.page_offset(), 0);

        page.handle_action(Action::PageRight);
        assert_eq!(page.page_offset(), 20);

        // Simulate receiving page 1
        let wfs2: Vec<WorkflowInfo> = (20..40)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs2, 60))));

        page.handle_action(Action::PageRight);
        assert_eq!(page.page_offset(), 40);
    }

    #[test]
    fn clamp_page_when_offset_past_end() {
        let mut page = FlowsPage::new();
        // Start on page 2 (offset 40)
        page.page = 2;
        // Server now says total is only 25 (pages 0 and 1 only)
        let wfs: Vec<WorkflowInfo> = (0..5)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 25))));

        // Page should be clamped to last valid page (1)
        assert_eq!(page.page, 1);
        // A re-fetch should be pending to get the correct page data
        assert!(page.pending_fetch);
    }

    #[test]
    fn quit_action() {
        let mut page = FlowsPage::new();
        assert_eq!(page.handle_action(Action::Quit), PageResult::Quit);
    }

    #[test]
    fn unhandled_action_ignored() {
        let mut page = FlowsPage::new();
        assert_eq!(page.handle_action(Action::Back), PageResult::Ignored);
    }

    #[test]
    fn tab_id_and_metadata() {
        let page = FlowsPage::new();
        assert_eq!(page.tab_id(), TabId::Flows);
        assert_eq!(page.title(), "Flows");
        assert_eq!(page.shortcut_char(), 'f');
    }

    #[test]
    fn entry_to_row_stalled() {
        let wf = make_workflow("wf-1", "ur-abc", true);
        let entry = FlowEntry {
            timestamps: parse_timestamps(&wf),
            workflow: wf,
        };
        let now = Utc::now();
        let row = entry_to_row(&entry, now);
        assert_eq!(row[7], "X");
    }

    #[test]
    fn entry_to_row_not_stalled() {
        let wf = make_workflow("wf-1", "ur-abc", false);
        let entry = FlowEntry {
            timestamps: parse_timestamps(&wf),
            workflow: wf,
        };
        let now = Utc::now();
        let row = entry_to_row(&entry, now);
        assert_eq!(row[7], "");
    }

    #[test]
    fn refresh_resets_to_loading() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok((wfs, 1))));
        assert!(!page.needs_data());

        let result = page.handle_action(Action::Refresh);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.needs_data());
        assert!(page.refreshing);
    }

    #[test]
    fn footer_commands_not_empty() {
        let page = FlowsPage::new();
        let keymap = Keymap::default();
        let cmds = page.footer_commands(&keymap);
        assert!(!cmds.is_empty());
    }

    #[test]
    fn cancel_flow_returns_ignored_for_app_handling() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok((wfs, 1))));

        // Cancel actions are handled at the app level, so the page returns Ignored.
        assert_eq!(page.handle_action(Action::CloseTicket), PageResult::Ignored);
        assert_eq!(page.handle_action(Action::CancelFlow), PageResult::Ignored);
    }

    #[test]
    fn selected_ticket_id_returns_correct_id() {
        let mut page = FlowsPage::new();
        let wfs = vec![
            make_workflow("wf-1", "ur-abc", false),
            make_workflow("wf-2", "ur-def", false),
        ];
        page.on_data(&DataPayload::Flows(Ok((wfs, 2))));

        assert_eq!(page.selected_ticket_id(), Some("ur-abc".to_string()));
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_ticket_id(), Some("ur-def".to_string()));
    }

    #[test]
    fn selected_ticket_id_none_when_empty() {
        let mut page = FlowsPage::new();
        page.on_data(&DataPayload::Flows(Ok((vec![], 0))));
        assert!(page.selected_ticket_id().is_none());
    }

    #[test]
    fn on_action_result_success_shows_banner() {
        let mut page = FlowsPage::new();
        page.active_status = Some(StatusMessage {
            text: "Cancelling...".to_string(),
            dismissable: false,
        });

        let result = ActionResult {
            result: Ok("Cancelled workflow for ur-abc".to_string()),
            silent_on_success: false,
        };
        page.on_action_result(&result);

        assert!(page.active_status.is_none());
        assert!(page.active_banner.is_some());
        let banner = page.active_banner.as_ref().unwrap();
        assert_eq!(banner.variant, BannerVariant::Success);
        assert_eq!(banner.message, "Cancelled workflow for ur-abc");
    }

    #[test]
    fn on_action_result_error_shows_banner() {
        let mut page = FlowsPage::new();
        let result = ActionResult {
            result: Err("server error".to_string()),
            silent_on_success: false,
        };
        page.on_action_result(&result);

        assert!(page.active_banner.is_some());
        let banner = page.active_banner.as_ref().unwrap();
        assert_eq!(banner.variant, BannerVariant::Error);
        assert_eq!(banner.message, "server error");
    }

    #[test]
    fn on_action_result_silent_success_no_banner() {
        let mut page = FlowsPage::new();
        let result = ActionResult {
            result: Ok("done".to_string()),
            silent_on_success: true,
        };
        page.on_action_result(&result);
        assert!(page.active_banner.is_none());
    }

    #[test]
    fn banner_dismiss() {
        let mut page = FlowsPage::new();
        page.active_banner = Some(Banner {
            message: "test".to_string(),
            variant: BannerVariant::Success,
            created_at: Instant::now(),
        });
        assert!(page.banner().is_some());
        page.dismiss_banner();
        assert!(page.banner().is_none());
    }

    #[test]
    fn footer_includes_cancel() {
        let page = FlowsPage::new();
        let keymap = Keymap::default();
        let cmds = page.footer_commands(&keymap);
        assert!(cmds.iter().any(|c| c.description == "Cancel"));
    }

    #[test]
    fn full_list_load_clears_and_rebuilds() {
        let mut page = FlowsPage::new();
        let batch1 = vec![
            make_workflow("wf-1", "ur-abc", false),
            make_workflow("wf-2", "ur-def", false),
        ];
        page.on_data(&DataPayload::Flows(Ok((batch1, 2))));
        assert_eq!(page.entry_map.len(), 2);

        // Full list load with different workflows replaces all
        let batch2 = vec![make_workflow("wf-3", "ur-ghi", false)];
        page.on_data(&DataPayload::Flows(Ok((batch2, 1))));
        assert_eq!(page.entry_map.len(), 1);
        assert!(page.entry_map.contains_key("ur-ghi"));
        assert!(!page.entry_map.contains_key("ur-abc"));
    }

    #[test]
    fn workflow_progress_with_children() {
        let mut wf = make_workflow("wf-1", "ur-abc", false);
        wf.ticket_children_open = 3;
        wf.ticket_children_closed = 7;
        let (completed, total) = workflow_progress(&wf);
        assert_eq!(completed, 7);
        assert_eq!(total, 10);
    }

    #[test]
    fn workflow_progress_terminal_no_children() {
        let mut wf = make_workflow("wf-1", "ur-abc", false);
        wf.status = "done".into();
        let (completed, total) = workflow_progress(&wf);
        assert_eq!(completed, 1);
        assert_eq!(total, 1);
    }

    #[test]
    fn workflow_progress_active_no_children() {
        let wf = make_workflow("wf-1", "ur-abc", false);
        let (completed, total) = workflow_progress(&wf);
        assert_eq!(completed, 0);
        assert_eq!(total, 1);
    }

    #[test]
    fn select_enters_detail_mode() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok((wfs, 1))));

        assert!(page.detail_workflow.is_none());
        let result = page.handle_action(Action::Select);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.detail_workflow.is_some());
        assert_eq!(page.detail_workflow.as_ref().unwrap().ticket_id, "ur-abc");
    }

    #[test]
    fn back_exits_detail_mode() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok((wfs, 1))));

        page.handle_action(Action::Select);
        assert!(page.detail_workflow.is_some());

        let result = page.handle_action(Action::Back);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.detail_workflow.is_none());
    }

    #[test]
    fn detail_mode_ignores_navigation() {
        let mut page = FlowsPage::new();
        let wfs = vec![
            make_workflow("wf-1", "ur-abc", false),
            make_workflow("wf-2", "ur-def", false),
        ];
        page.on_data(&DataPayload::Flows(Ok((wfs, 2))));

        page.handle_action(Action::Select);
        assert!(page.detail_workflow.is_some());

        // Navigation actions are consumed but do nothing meaningful
        let result = page.handle_action(Action::NavigateDown);
        assert_eq!(result, PageResult::Consumed);
    }

    #[test]
    fn detail_mode_quit_works() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok((wfs, 1))));

        page.handle_action(Action::Select);
        let result = page.handle_action(Action::Quit);
        assert_eq!(result, PageResult::Quit);
    }

    #[test]
    fn select_noop_when_empty() {
        let mut page = FlowsPage::new();
        page.on_data(&DataPayload::Flows(Ok((vec![], 0))));

        let result = page.handle_action(Action::Select);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.detail_workflow.is_none());
    }

    #[test]
    fn footer_includes_select_in_list_mode() {
        let page = FlowsPage::new();
        let keymap = Keymap::default();
        let cmds = page.footer_commands(&keymap);
        assert!(cmds.iter().any(|c| c.description == "Select"));
    }

    #[test]
    fn footer_in_detail_mode_has_back_and_quit() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok((wfs, 1))));
        page.handle_action(Action::Select);

        let keymap = Keymap::default();
        let cmds = page.footer_commands(&keymap);
        assert_eq!(cmds.len(), 2);
        assert!(cmds.iter().any(|c| c.description == "Back"));
        assert!(cmds.iter().any(|c| c.description == "Quit"));
    }

    #[test]
    fn pending_fetch_cleared_on_data_receipt() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..20)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 45))));

        page.handle_action(Action::PageRight);
        assert!(page.pending_fetch);

        // Receiving data clears pending_fetch
        let wfs2: Vec<WorkflowInfo> = (20..40)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs2, 45))));
        assert!(!page.pending_fetch);
        assert!(!page.needs_data());
    }

    #[test]
    fn only_holds_one_page_of_data() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..20)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs, 45))));
        assert_eq!(page.entry_map.len(), 20);

        // Navigate to page 1 and load new data
        page.handle_action(Action::PageRight);
        let wfs2: Vec<WorkflowInfo> = (20..40)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i:02}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok((wfs2, 45))));

        // Should only hold page 1 data, not both pages
        assert_eq!(page.entry_map.len(), 20);
        assert!(!page.entry_map.contains_key("ur-00"));
        assert!(page.entry_map.contains_key("ur-20"));
    }
}
