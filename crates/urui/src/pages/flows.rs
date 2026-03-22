use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use ur_rpc::proto::ticket::WorkflowInfo;

use crate::context::TuiContext;
use crate::data::DataPayload;
use crate::keymap::Action;
use crate::page::{FooterCommand, Page, PageResult, TabId};
use crate::widgets::ThemedTable;

const PAGE_SIZE: usize = 20;

/// State for the Flows tab, showing all workflows in a paginated table.
pub struct FlowsPage {
    workflows: Vec<WorkflowInfo>,
    selected: usize,
    page: usize,
    loaded: bool,
    error: Option<String>,
}

impl FlowsPage {
    pub fn new() -> Self {
        Self {
            workflows: Vec::new(),
            selected: 0,
            page: 0,
            loaded: false,
            error: None,
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

        let rows: Vec<Vec<String>> = self.page_rows().iter().map(workflow_to_row).collect();

        let selected = if rows.is_empty() {
            None
        } else {
            Some(self.selected)
        };

        let page_info = format!("Page {}/{}", self.page + 1, self.total_pages());

        let table = ThemedTable {
            headers: vec![
                "Ticket ID",
                "Workflow ID",
                "Status",
                "Cycles",
                "PR URL",
                "Stalled",
            ],
            rows,
            selected,
            widths: vec![
                Constraint::Percentage(15),
                Constraint::Percentage(15),
                Constraint::Percentage(15),
                Constraint::Percentage(8),
                Constraint::Percentage(37),
                Constraint::Percentage(10),
            ],
            page_info: Some(page_info),
        };

        table.render(area, buf, ctx);
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "j".into(),
                description: "Down".into(),
            },
            FooterCommand {
                key_label: "k".into(),
                description: "Up".into(),
            },
            FooterCommand {
                key_label: "h/l".into(),
                description: "Page".into(),
            },
            FooterCommand {
                key_label: "r".into(),
                description: "Refresh".into(),
            },
            FooterCommand {
                key_label: "q".into(),
                description: "Back".into(),
            },
            FooterCommand {
                key_label: "Q".into(),
                description: "Quit".into(),
            },
        ]
    }

    fn on_data(&mut self, payload: &DataPayload) {
        if let DataPayload::Flows(result) = payload {
            self.loaded = true;
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
        !self.loaded
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
        wf.id.clone(),
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
        assert_eq!(row[5], "!! test stall");
    }

    #[test]
    fn workflow_to_row_not_stalled() {
        let wf = make_workflow("wf-1", "ur-abc", false);
        let row = workflow_to_row(&wf);
        assert_eq!(row[5], "");
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
        assert!(!page.loaded);
    }

    #[test]
    fn footer_commands_not_empty() {
        let page = FlowsPage::new();
        let cmds = page.footer_commands();
        assert!(!cmds.is_empty());
    }
}
