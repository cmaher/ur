use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use ur_rpc::proto::ticket::WorkflowInfo;

use crate::context::TuiContext;
use crate::data::{ActionResult, DataPayload};
use crate::keymap::{Action, Keymap};
use crate::page::{Banner, BannerVariant, FooterCommand, Page, PageResult, StatusMessage, TabId};
use crate::widgets::ThemedTable;

const PAGE_SIZE: usize = 20;

/// State for the Flows tab, showing all workflows in a paginated table.
pub struct FlowsPage {
    workflows: Vec<WorkflowInfo>,
    selected: usize,
    page: usize,
    loaded: bool,
    error: Option<String>,
    refreshing: bool,
    /// In-progress status message shown below the tab header.
    active_status: Option<StatusMessage>,
    /// Active notification banner (success/error from async actions).
    active_banner: Option<Banner>,
    /// Ticket ID for which a cancel was requested but not yet dispatched.
    pending_cancel: Option<String>,
}

impl FlowsPage {
    pub fn new() -> Self {
        Self {
            workflows: Vec::new(),
            selected: 0,
            page: 0,
            loaded: false,
            error: None,
            refreshing: false,
            active_status: None,
            active_banner: None,
            pending_cancel: None,
        }
    }

    fn total_pages(&self) -> usize {
        if self.workflows.is_empty() {
            1
        } else {
            self.workflows.len().div_ceil(PAGE_SIZE)
        }
    }

    fn page_rows(&self) -> &[WorkflowInfo] {
        let start = self.page * PAGE_SIZE;
        let end = (start + PAGE_SIZE).min(self.workflows.len());
        if start >= self.workflows.len() {
            &[]
        } else {
            &self.workflows[start..end]
        }
    }

    fn page_row_count(&self) -> usize {
        self.page_rows().len()
    }

    fn clamp_selection(&mut self) {
        let count = self.page_row_count();
        if count == 0 {
            self.selected = 0;
        } else if self.selected >= count {
            self.selected = count - 1;
        }
    }

    /// Returns the ticket ID of the currently selected workflow, if any.
    pub fn selected_ticket_id(&self) -> Option<String> {
        let rows = self.page_rows();
        rows.get(self.selected).map(|wf| wf.ticket_id.clone())
    }

    /// Take the pending cancel ticket ID, if one was set by the user pressing X.
    /// Calling this clears the pending cancel so it is only dispatched once.
    pub fn take_pending_cancel(&mut self) -> Option<String> {
        self.pending_cancel.take()
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
                self.refreshing = true;
                self.active_status = Some(StatusMessage {
                    text: "Refreshing flows...".to_string(),
                    dismissable: true,
                });
                PageResult::Consumed
            }
            Action::CancelFlow | Action::CloseTicket => {
                if let Some(ticket_id) = self.selected_ticket_id() {
                    self.active_status = Some(StatusMessage {
                        text: format!("Cancelling workflow for {ticket_id}..."),
                        dismissable: false,
                    });
                    self.pending_cancel = Some(ticket_id);
                }
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

        let rows: Vec<Vec<String>> = self.page_rows().iter().map(workflow_to_row).collect();

        let selected = if rows.is_empty() {
            None
        } else {
            Some(self.selected)
        };

        let page_info = format!("Page {}/{}", self.page + 1, self.total_pages());

        let table = ThemedTable {
            headers: vec!["Ticket ID", "Status", "Cycles", "PR URL", "Stalled"],
            rows,
            selected,
            widths: vec![
                Constraint::Length(12),
                Constraint::Length(20),
                Constraint::Length(8),
                Constraint::Length(45),
                Constraint::Fill(1),
            ],
            page_info: Some(page_info),
        };

        table.render(area, buf, ctx);
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
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
        if let DataPayload::Flows(result) = payload {
            self.loaded = true;
            self.refreshing = false;
            self.active_status = None;
            match result {
                Ok(workflows) => {
                    self.workflows = workflows.clone();
                    self.error = None;
                    self.clamp_selection();
                }
                Err(msg) => {
                    self.error = Some(msg.clone());
                    self.workflows.clear();
                }
            }
        }
    }

    fn needs_data(&self) -> bool {
        !self.loaded || self.refreshing
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
        self.loaded = false;
    }
}

/// Convert a WorkflowInfo into a row of display strings.
fn workflow_to_row(wf: &WorkflowInfo) -> Vec<String> {
    let stalled_text = if wf.stalled {
        format!("!! {}", wf.stall_reason)
    } else {
        String::new()
    };

    vec![
        wf.ticket_id.clone(),
        wf.status.clone(),
        wf.implement_cycles.to_string(),
        wf.pr_url.clone(),
        stalled_text,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

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
        }
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
        page.on_data(&DataPayload::Flows(Ok(wfs.clone())));
        assert!(page.loaded);
        assert!(!page.needs_data());
        assert_eq!(page.workflows.len(), 1);
        assert!(page.error.is_none());
    }

    #[test]
    fn on_data_handles_error() {
        let mut page = FlowsPage::new();
        page.on_data(&DataPayload::Flows(Err("connection refused".into())));
        assert!(page.loaded);
        assert!(page.error.is_some());
        assert!(page.workflows.is_empty());
    }

    #[test]
    fn on_data_ignores_tickets_payload() {
        let mut page = FlowsPage::new();
        page.on_data(&DataPayload::Tickets(Ok(vec![])));
        assert!(!page.loaded);
    }

    #[test]
    fn navigate_up_down() {
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..3)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok(wfs)));

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
        let mut page = FlowsPage::new();
        let wfs: Vec<WorkflowInfo> = (0..45)
            .map(|i| make_workflow(&format!("wf-{i}"), &format!("ur-{i}"), false))
            .collect();
        page.on_data(&DataPayload::Flows(Ok(wfs)));

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
        let mut page = FlowsPage::new();
        assert_eq!(page.handle_action(Action::Quit), PageResult::Quit);
    }

    #[test]
    fn unhandled_action_ignored() {
        let mut page = FlowsPage::new();
        assert_eq!(page.handle_action(Action::Select), PageResult::Ignored);
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
    fn workflow_to_row_stalled() {
        let wf = make_workflow("wf-1", "ur-abc", true);
        let row = workflow_to_row(&wf);
        assert_eq!(row[4], "!! test stall");
    }

    #[test]
    fn workflow_to_row_not_stalled() {
        let wf = make_workflow("wf-1", "ur-abc", false);
        let row = workflow_to_row(&wf);
        assert_eq!(row[4], "");
    }

    #[test]
    fn refresh_resets_to_loading() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok(wfs)));
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
    fn cancel_flow_sets_pending_and_status() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok(wfs)));

        let result = page.handle_action(Action::CloseTicket);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.active_status.is_some());
        assert_eq!(
            page.active_status.as_ref().unwrap().text,
            "Cancelling workflow for ur-abc..."
        );
        assert_eq!(page.pending_cancel, Some("ur-abc".to_string()));
    }

    #[test]
    fn cancel_flow_action_sets_pending_and_status() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok(wfs)));

        let result = page.handle_action(Action::CancelFlow);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.active_status.is_some());
        assert_eq!(page.pending_cancel, Some("ur-abc".to_string()));
    }

    #[test]
    fn cancel_flow_noop_when_empty() {
        let mut page = FlowsPage::new();
        page.on_data(&DataPayload::Flows(Ok(vec![])));

        let result = page.handle_action(Action::CloseTicket);
        assert_eq!(result, PageResult::Consumed);
        assert!(page.pending_cancel.is_none());
        assert!(page.active_status.is_none());
    }

    #[test]
    fn take_pending_cancel_clears() {
        let mut page = FlowsPage::new();
        let wfs = vec![make_workflow("wf-1", "ur-abc", false)];
        page.on_data(&DataPayload::Flows(Ok(wfs)));

        page.handle_action(Action::CloseTicket);
        let taken = page.take_pending_cancel();
        assert_eq!(taken, Some("ur-abc".to_string()));
        assert!(page.take_pending_cancel().is_none());
    }

    #[test]
    fn selected_ticket_id_returns_correct_id() {
        let mut page = FlowsPage::new();
        let wfs = vec![
            make_workflow("wf-1", "ur-abc", false),
            make_workflow("wf-2", "ur-def", false),
        ];
        page.on_data(&DataPayload::Flows(Ok(wfs)));

        assert_eq!(page.selected_ticket_id(), Some("ur-abc".to_string()));
        page.handle_action(Action::NavigateDown);
        assert_eq!(page.selected_ticket_id(), Some("ur-def".to_string()));
    }

    #[test]
    fn selected_ticket_id_none_when_empty() {
        let mut page = FlowsPage::new();
        page.on_data(&DataPayload::Flows(Ok(vec![])));
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
}
