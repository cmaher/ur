use std::collections::HashMap;
use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use ur_rpc::proto::core::WorkerSummary;

use crate::context::TuiContext;
use crate::data::{ActionResult, DataPayload};
use crate::keymap::{Action, Keymap};
use crate::page::{Banner, BannerVariant, FooterCommand, Page, PageResult, TabId};
use crate::widgets::ThemedTable;

const PAGE_SIZE: usize = 20;

/// State for the Workers tab, showing active workers in a paginated table.
pub struct WorkersPage {
    /// Map of worker_id -> WorkerSummary for efficient lookups.
    entry_map: HashMap<String, WorkerSummary>,
    /// Sorted display list of worker IDs, rebuilt on data changes.
    display_ids: Vec<String>,
    /// Workers optimistically removed while a kill RPC is in flight.
    /// Restored on error; cleared when fresh server data arrives.
    pending_kills: HashMap<String, WorkerSummary>,
    selected: usize,
    page: usize,
    loaded: bool,
    error: Option<String>,
    active_banner: Option<Banner>,
}

impl WorkersPage {
    pub fn new() -> Self {
        Self {
            entry_map: HashMap::new(),
            display_ids: Vec::new(),
            pending_kills: HashMap::new(),
            selected: 0,
            page: 0,
            loaded: false,
            error: None,
            active_banner: None,
        }
    }

    fn total_pages(&self) -> usize {
        if self.display_ids.is_empty() {
            1
        } else {
            self.display_ids.len().div_ceil(PAGE_SIZE)
        }
    }

    fn page_ids(&self) -> &[String] {
        let start = self.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.display_ids.len());
        if start >= self.display_ids.len() {
            &[]
        } else {
            &self.display_ids[start..end]
        }
    }

    fn page_entries(&self) -> Vec<&WorkerSummary> {
        self.page_ids()
            .iter()
            .filter_map(|id| self.entry_map.get(id))
            .collect()
    }

    fn page_row_count(&self) -> usize {
        self.page_ids().len()
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
    }

    /// Preserve selection by worker ID, rebuild display list, restore selection.
    fn preserve_selection_and_rebuild(&mut self) {
        let selected_id = self.selected_worker_id();
        self.rebuild_display_ids();
        self.restore_selection_by_id(selected_id.as_deref());
    }

    /// Restore selection to a worker ID, or clamp if the ID is gone.
    fn restore_selection_by_id(&mut self, worker_id: Option<&str>) {
        if let Some(id) = worker_id
            && let Some(pos) = self.display_ids.iter().position(|wid| wid == id)
        {
            self.page = pos / PAGE_SIZE;
            self.selected = pos % PAGE_SIZE;
            return;
        }
        self.clamp_selection();
    }

    /// Returns the worker ID of the currently selected worker, if any.
    pub fn selected_worker_id(&self) -> Option<String> {
        self.page_ids().get(self.selected).cloned()
    }

    /// Optimistically remove a worker from the display list while a kill RPC
    /// is in flight. The worker is stashed in `pending_kills` so it can be
    /// restored if the kill fails.
    pub fn optimistic_remove(&mut self, worker_id: &str) {
        if let Some(worker) = self.entry_map.remove(worker_id) {
            self.pending_kills.insert(worker_id.to_string(), worker);
            self.preserve_selection_and_rebuild();
        }
    }

    /// Handle an async action result by showing a success or error banner.
    /// On error, restores any optimistically removed workers.
    pub fn on_action_result(&mut self, result: &ActionResult) {
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
                // Restore optimistically removed workers on failure.
                if !self.pending_kills.is_empty() {
                    self.entry_map
                        .extend(std::mem::take(&mut self.pending_kills));
                    self.preserve_selection_and_rebuild();
                }
                self.active_banner = Some(Banner {
                    message: msg.clone(),
                    variant: BannerVariant::Error,
                    created_at: Instant::now(),
                });
            }
        }
    }
}

/// Convert a WorkerSummary into a row of display strings.
fn entry_to_row(worker: &WorkerSummary) -> Vec<String> {
    vec![
        worker.worker_id.clone(),
        worker.project_key.clone(),
        worker.mode.clone(),
        worker.container_status.clone(),
        worker.agent_status.clone(),
        worker.container_id.clone(),
    ]
}

impl Page for WorkersPage {
    fn tab_id(&self) -> TabId {
        TabId::Workers
    }

    fn title(&self) -> &str {
        "Workers"
    }

    fn shortcut_char(&self) -> char {
        'w'
    }

    fn handle_action(&mut self, action: Action) -> PageResult {
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
                }
                PageResult::Consumed
            }
            Action::PageRight => {
                if self.page + 1 < self.total_pages() {
                    self.page += 1;
                    self.selected = 0;
                }
                PageResult::Consumed
            }
            Action::Refresh => {
                self.loaded = false;
                PageResult::Consumed
            }
            Action::Quit => PageResult::Quit,
            _ => PageResult::Ignored,
        }
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

        let rows: Vec<Vec<String>> = self.page_entries().into_iter().map(entry_to_row).collect();

        let selected = if rows.is_empty() {
            None
        } else {
            Some(self.selected)
        };

        let page_info = format!("Page {}/{}", self.page + 1, self.total_pages());

        let widths = vec![
            Constraint::Length(16), // ID
            Constraint::Length(10), // Project
            Constraint::Length(8),  // Mode
            Constraint::Length(14), // Status
            Constraint::Length(10), // Agent
            Constraint::Fill(1),    // Container
        ];

        let table = ThemedTable {
            headers: vec!["ID", "Project", "Mode", "Status", "Agent", "Container"],
            rows,
            selected,
            widths,
            page_info: Some(page_info),
        };

        table.render(area, buf, ctx);
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: keymap.label_for(&Action::CloseTicket),
                description: "Kill".to_string(),
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

    fn banner(&self) -> Option<&Banner> {
        self.active_banner.as_ref()
    }

    fn dismiss_banner(&mut self) {
        self.active_banner = None;
    }

    fn tick_banner(&mut self) {
        if self.active_banner.as_ref().is_some_and(|b| b.is_expired()) {
            self.active_banner = None;
        }
    }

    fn on_data(&mut self, payload: &DataPayload) {
        match payload {
            DataPayload::Workers(Ok(workers)) => {
                self.loaded = true;
                self.pending_kills.clear();
                self.entry_map.clear();
                for w in workers {
                    self.entry_map.insert(w.worker_id.clone(), w.clone());
                }
                self.error = None;
                self.preserve_selection_and_rebuild();
            }
            DataPayload::Workers(Err(msg)) => {
                self.loaded = true;
                self.error = Some(msg.clone());
                self.entry_map.clear();
                self.display_ids.clear();
            }
            _ => {}
        }
    }

    fn needs_data(&self) -> bool {
        !self.loaded
    }

    fn mark_stale(&mut self) {
        self.loaded = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_worker(worker_id: &str) -> WorkerSummary {
        WorkerSummary {
            worker_id: worker_id.into(),
            worker_id_full: format!("{worker_id}-full"),
            container_id: "ctr-abc123".into(),
            project_key: "ur".into(),
            mode: "implement".into(),
            grpc_port: 50051,
            directory: "/workspace".into(),
            container_status: "running".into(),
            agent_status: "active".into(),
            lifecycle_status: String::new(),
            stall_reason: String::new(),
            pr_url: String::new(),
            workflow_id: String::new(),
            workflow_status: String::new(),
            workflow_stalled: false,
            workflow_stall_reason: String::new(),
        }
    }

    #[test]
    fn new_page_needs_data() {
        let page = WorkersPage::new();
        assert!(page.needs_data());
        assert!(!page.loaded);
    }

    #[test]
    fn tab_id_and_metadata() {
        let page = WorkersPage::new();
        assert_eq!(page.tab_id(), TabId::Workers);
        assert_eq!(page.title(), "Workers");
        assert_eq!(page.shortcut_char(), 'w');
    }

    #[test]
    fn on_data_loads_workers() {
        let mut page = WorkersPage::new();
        let workers = vec![make_worker("ur-abc")];
        page.on_data(&DataPayload::Workers(Ok(workers)));
        assert!(page.loaded);
        assert!(!page.needs_data());
        assert_eq!(page.entry_map.len(), 1);
        assert!(page.error.is_none());
    }

    #[test]
    fn on_data_handles_error() {
        let mut page = WorkersPage::new();
        page.on_data(&DataPayload::Workers(Err("connection refused".into())));
        assert!(page.loaded);
        assert!(page.error.is_some());
        assert!(page.entry_map.is_empty());
    }

    #[test]
    fn on_data_ignores_tickets_payload() {
        let mut page = WorkersPage::new();
        page.on_data(&DataPayload::Tickets(Ok((vec![], 0))));
        assert!(!page.loaded);
    }

    #[test]
    fn navigate_up_down() {
        let mut page = WorkersPage::new();
        let workers: Vec<WorkerSummary> = (0..3).map(|i| make_worker(&format!("ur-{i}"))).collect();
        page.on_data(&DataPayload::Workers(Ok(workers)));

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
    fn pagination() {
        let mut page = WorkersPage::new();
        let workers: Vec<WorkerSummary> = (0..45)
            .map(|i| make_worker(&format!("ur-{i:02}")))
            .collect();
        page.on_data(&DataPayload::Workers(Ok(workers)));

        assert_eq!(page.total_pages(), 3);
        assert_eq!(page.page, 0);
        assert_eq!(page.page_row_count(), 20);

        assert_eq!(page.handle_action(Action::PageRight), PageResult::Consumed);
        assert_eq!(page.page, 1);
        assert_eq!(page.selected, 0);
        assert_eq!(page.page_row_count(), 20);

        assert_eq!(page.handle_action(Action::PageRight), PageResult::Consumed);
        assert_eq!(page.page, 2);
        assert_eq!(page.page_row_count(), 5);

        // Can't go past last page
        assert_eq!(page.handle_action(Action::PageRight), PageResult::Consumed);
        assert_eq!(page.page, 2);

        assert_eq!(page.handle_action(Action::PageLeft), PageResult::Consumed);
        assert_eq!(page.page, 1);
    }

    #[test]
    fn quit_action() {
        let mut page = WorkersPage::new();
        assert_eq!(page.handle_action(Action::Quit), PageResult::Quit);
    }

    #[test]
    fn unhandled_action_ignored() {
        let mut page = WorkersPage::new();
        assert_eq!(page.handle_action(Action::Select), PageResult::Ignored);
        assert_eq!(page.handle_action(Action::Back), PageResult::Ignored);
    }

    #[test]
    fn full_list_load_clears_and_rebuilds() {
        let mut page = WorkersPage::new();
        let batch1 = vec![make_worker("ur-abc"), make_worker("ur-def")];
        page.on_data(&DataPayload::Workers(Ok(batch1)));
        assert_eq!(page.entry_map.len(), 2);

        // Full list load with different workers replaces all
        let batch2 = vec![make_worker("ur-ghi")];
        page.on_data(&DataPayload::Workers(Ok(batch2)));
        assert_eq!(page.entry_map.len(), 1);
        assert!(page.entry_map.contains_key("ur-ghi"));
        assert!(!page.entry_map.contains_key("ur-abc"));
    }

    #[test]
    fn selection_preserved_across_rebuild() {
        let mut page = WorkersPage::new();
        let workers = vec![
            make_worker("ur-abc"),
            make_worker("ur-def"),
            make_worker("ur-ghi"),
        ];
        page.on_data(&DataPayload::Workers(Ok(workers)));

        // Select ur-def (sorted: ur-abc=0, ur-def=1, ur-ghi=2)
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_worker_id(), Some("ur-def".to_string()));

        // Full reload -- selection preserved by ID
        let workers2 = vec![
            make_worker("ur-abc"),
            make_worker("ur-def"),
            make_worker("ur-ghi"),
        ];
        page.on_data(&DataPayload::Workers(Ok(workers2)));
        assert_eq!(page.selected_worker_id(), Some("ur-def".to_string()));
    }

    #[test]
    fn selection_clamped_when_id_disappears() {
        let mut page = WorkersPage::new();
        let workers = vec![
            make_worker("ur-abc"),
            make_worker("ur-def"),
            make_worker("ur-ghi"),
        ];
        page.on_data(&DataPayload::Workers(Ok(workers)));

        // Select ur-ghi (index 2)
        page.handle_action(Action::NavigateDown);
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_worker_id(), Some("ur-ghi".to_string()));

        // Reload without ur-ghi -- selection clamped
        let workers2 = vec![make_worker("ur-abc"), make_worker("ur-def")];
        page.on_data(&DataPayload::Workers(Ok(workers2)));
        assert!(page.selected_worker_id().is_some());
    }

    #[test]
    fn refresh_resets_to_loading() {
        let mut page = WorkersPage::new();
        let workers = vec![make_worker("ur-abc")];
        page.on_data(&DataPayload::Workers(Ok(workers)));
        assert!(!page.needs_data());

        let result = page.handle_action(Action::Refresh);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.needs_data());
        assert!(!page.loaded);
    }

    #[test]
    fn footer_commands_not_empty() {
        let page = WorkersPage::new();
        let keymap = Keymap::default();
        let cmds = page.footer_commands(&keymap);
        assert!(!cmds.is_empty());
    }

    #[test]
    fn selected_worker_id_none_when_empty() {
        let mut page = WorkersPage::new();
        page.on_data(&DataPayload::Workers(Ok(vec![])));
        assert!(page.selected_worker_id().is_none());
    }

    #[test]
    fn entry_to_row_fields() {
        let worker = make_worker("ur-abc");
        let row = entry_to_row(&worker);
        assert_eq!(row[0], "ur-abc");
        assert_eq!(row[1], "ur");
        assert_eq!(row[2], "implement");
        assert_eq!(row[3], "running");
        assert_eq!(row[4], "active");
        assert_eq!(row[5], "ctr-abc123");
    }

    #[test]
    fn mark_stale_triggers_refetch() {
        let mut page = WorkersPage::new();
        let workers = vec![make_worker("ur-abc")];
        page.on_data(&DataPayload::Workers(Ok(workers)));
        assert!(!page.needs_data());

        page.mark_stale();
        assert!(page.needs_data());
    }

    #[test]
    fn optimistic_remove_hides_worker_immediately() {
        let mut page = WorkersPage::new();
        let workers = vec![
            make_worker("ur-abc"),
            make_worker("ur-def"),
            make_worker("ur-ghi"),
        ];
        page.on_data(&DataPayload::Workers(Ok(workers)));
        assert_eq!(page.entry_map.len(), 3);

        page.optimistic_remove("ur-def");
        assert_eq!(page.entry_map.len(), 2);
        assert!(!page.entry_map.contains_key("ur-def"));
        assert_eq!(page.display_ids.len(), 2);
        assert!(page.pending_kills.contains_key("ur-def"));
    }

    #[test]
    fn optimistic_remove_restores_on_error() {
        let mut page = WorkersPage::new();
        let workers = vec![make_worker("ur-abc"), make_worker("ur-def")];
        page.on_data(&DataPayload::Workers(Ok(workers)));

        page.optimistic_remove("ur-abc");
        assert_eq!(page.entry_map.len(), 1);

        // Simulate kill failure
        let result = ActionResult {
            result: Err("connection refused".into()),
            silent_on_success: false,
        };
        page.on_action_result(&result);

        assert_eq!(page.entry_map.len(), 2);
        assert!(page.entry_map.contains_key("ur-abc"));
        assert!(page.pending_kills.is_empty());
    }

    #[test]
    fn optimistic_remove_cleared_on_fresh_data() {
        let mut page = WorkersPage::new();
        let workers = vec![make_worker("ur-abc"), make_worker("ur-def")];
        page.on_data(&DataPayload::Workers(Ok(workers)));

        page.optimistic_remove("ur-abc");
        assert!(!page.pending_kills.is_empty());

        // Fresh server data clears pending kills
        let fresh = vec![make_worker("ur-def")];
        page.on_data(&DataPayload::Workers(Ok(fresh)));
        assert!(page.pending_kills.is_empty());
        assert_eq!(page.entry_map.len(), 1);
    }

    #[test]
    fn optimistic_remove_noop_for_unknown_worker() {
        let mut page = WorkersPage::new();
        let workers = vec![make_worker("ur-abc")];
        page.on_data(&DataPayload::Workers(Ok(workers)));

        page.optimistic_remove("ur-unknown");
        assert_eq!(page.entry_map.len(), 1);
        assert!(page.pending_kills.is_empty());
    }

    #[test]
    fn optimistic_remove_clamps_selection() {
        let mut page = WorkersPage::new();
        let workers = vec![make_worker("ur-abc"), make_worker("ur-def")];
        page.on_data(&DataPayload::Workers(Ok(workers)));

        // Select the last worker (ur-def at index 1)
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_worker_id(), Some("ur-def".to_string()));

        // Remove the last worker — selection should clamp
        page.optimistic_remove("ur-def");
        assert_eq!(page.display_ids.len(), 1);
        assert!(page.selected_worker_id().is_some());
        assert_eq!(page.selected_worker_id(), Some("ur-abc".to_string()));
    }
}
