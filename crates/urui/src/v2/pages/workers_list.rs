use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use crate::context::TuiContext;
use crate::v2::cmd::{Cmd, FetchCmd};
use crate::v2::input::{FooterCommand, InputHandler, InputResult};
use crate::v2::model::{LoadState, Model, WORKER_PAGE_SIZE, WorkerListData, WorkerListModel};
use crate::v2::msg::{GotoTarget, Msg, NavMsg, OverlayMsg};
use crate::widgets::ThemedTable;

use ur_rpc::proto::core::WorkerSummary;

/// Render the workers list page into the given content area.
///
/// Shows a table of active workers with ID, Project, Mode, Status, Agent,
/// and Container columns. Supports pagination.
pub fn render_workers_list(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    match &model.worker_list.data {
        LoadState::NotLoaded | LoadState::Loading => {
            render_message(area, buf, ctx, "Loading...");
        }
        LoadState::Error(msg) => {
            render_message(area, buf, ctx, &format!("Error: {msg}"));
        }
        LoadState::Loaded(data) => {
            render_loaded_workers(area, buf, ctx, &model.worker_list, data);
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

/// Render the workers table when data is loaded.
fn render_loaded_workers(
    area: Rect,
    buf: &mut Buffer,
    ctx: &TuiContext,
    worker_model: &WorkerListModel,
    data: &WorkerListData,
) {
    let sorted = sorted_workers(&data.workers);
    let total = sorted.len();
    let total_pages = total_pages(total);
    let page = worker_model.current_page;

    let page_workers = page_slice(&sorted, page);

    let rows: Vec<Vec<String>> = page_workers.iter().map(|w| worker_to_row(w)).collect();

    let selected = if rows.is_empty() {
        None
    } else {
        Some(worker_model.selected_row)
    };

    let page_info = format!("Page {}/{}", page + 1, total_pages);

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

/// Convert a `WorkerSummary` into a row of display strings.
fn worker_to_row(worker: &WorkerSummary) -> Vec<String> {
    vec![
        worker.worker_id.clone(),
        worker.project_key.clone(),
        worker.mode.clone(),
        worker.container_status.clone(),
        worker.agent_status.clone(),
        worker.container_id.clone(),
    ]
}

/// Sort workers by worker_id for stable display order.
fn sorted_workers(workers: &[WorkerSummary]) -> Vec<&WorkerSummary> {
    let mut sorted: Vec<&WorkerSummary> = workers.iter().collect();
    sorted.sort_by(|a, b| a.worker_id.cmp(&b.worker_id));
    sorted
}

/// Calculate the total number of pages.
fn total_pages(total: usize) -> usize {
    if total == 0 {
        1
    } else {
        total.div_ceil(WORKER_PAGE_SIZE)
    }
}

/// Get the workers for the current page.
fn page_slice<'a>(sorted: &[&'a WorkerSummary], page: usize) -> Vec<&'a WorkerSummary> {
    let start = page * WORKER_PAGE_SIZE;
    let end = (start + WORKER_PAGE_SIZE).min(sorted.len());
    if start >= sorted.len() {
        vec![]
    } else {
        sorted[start..end].to_vec()
    }
}

/// Get the worker ID of the currently selected worker, if any.
pub fn selected_worker_id(model: &Model) -> Option<String> {
    let data = model.worker_list.data.data()?;
    let sorted = sorted_workers(&data.workers);
    let start = model.worker_list.current_page * WORKER_PAGE_SIZE;
    let end = (start + WORKER_PAGE_SIZE).min(sorted.len());
    if start >= sorted.len() {
        return None;
    }
    let page_workers = &sorted[start..end];
    let idx = model.worker_list.selected_row;
    page_workers.get(idx).map(|w| w.worker_id.clone())
}

/// Handle worker list navigation messages. Called from the update function.
pub fn handle_workers_nav(mut model: Model, nav_msg: NavMsg) -> (Model, Vec<Cmd>) {
    match nav_msg {
        NavMsg::WorkersNavigate { delta } => {
            workers_navigate(&mut model, delta);
            (model, vec![])
        }
        NavMsg::WorkersPageRight => {
            workers_page_right(&mut model);
            (model, vec![])
        }
        NavMsg::WorkersPageLeft => {
            workers_page_left(&mut model);
            (model, vec![])
        }
        NavMsg::WorkersRefresh => {
            model.worker_list.data = LoadState::NotLoaded;
            model.worker_list.selected_row = 0;
            model.worker_list.current_page = 0;
            let cmd = Cmd::Fetch(FetchCmd::Workers);
            model.worker_list.data = LoadState::Loading;
            (model, vec![cmd])
        }
        NavMsg::WorkersKill => {
            let cmd = handle_kill(&mut model);
            (model, vec![cmd])
        }
        NavMsg::WorkersGoto => {
            handle_goto(&mut model);
            (model, vec![])
        }
        _ => (model, vec![]),
    }
}

/// Navigate within the workers table by delta.
fn workers_navigate(model: &mut Model, delta: i32) {
    let page_count = current_page_count(model);
    if page_count == 0 {
        return;
    }
    let new = (model.worker_list.selected_row as i32 + delta)
        .max(0)
        .min(page_count as i32 - 1) as usize;
    model.worker_list.selected_row = new;
}

/// Move to the next page.
fn workers_page_right(model: &mut Model) {
    let total = worker_count(model);
    let tp = total_pages(total);
    if model.worker_list.current_page + 1 < tp {
        model.worker_list.current_page += 1;
        model.worker_list.selected_row = 0;
    }
}

/// Move to the previous page.
fn workers_page_left(model: &mut Model) {
    if model.worker_list.current_page > 0 {
        model.worker_list.current_page -= 1;
        model.worker_list.selected_row = 0;
    }
}

/// Handle the kill action: optimistically remove the selected worker and
/// return a StopWorker command. If no worker is selected, returns Cmd::None.
fn handle_kill(model: &mut Model) -> Cmd {
    let Some(worker_id) = selected_worker_id(model) else {
        return Cmd::None;
    };

    // Optimistically remove the worker from the data
    if let LoadState::Loaded(ref mut data) = model.worker_list.data {
        data.workers.retain(|w| w.worker_id != worker_id);
    }
    clamp_selection(model);

    Cmd::StopWorker { worker_id }
}

/// Handle the goto action: open a goto menu with targets for the selected worker.
fn handle_goto(model: &mut Model) {
    let Some(worker_id) = selected_worker_id(model) else {
        return;
    };

    let targets = vec![
        GotoTarget {
            label: "Ticket Details".to_string(),
            screen: "ticket".to_string(),
            id: worker_id.clone(),
        },
        GotoTarget {
            label: "Flow Details".to_string(),
            screen: "flow".to_string(),
            id: worker_id,
        },
    ];

    model.active_overlay = Some(crate::v2::model::ActiveOverlay::GotoMenu { targets, cursor: 0 });
}

/// Handle a WorkerStopped result. On error, show an error banner and re-fetch.
pub fn handle_worker_stopped(
    model: Model,
    worker_id: String,
    result: Result<(), String>,
) -> (Model, Vec<Cmd>) {
    match result {
        Ok(()) => {
            // Success: show banner and re-fetch to get authoritative state
            let (model, mut cmds) = super::super::update::update(
                model,
                Msg::BannerShow {
                    message: format!("Killed {worker_id}"),
                    variant: crate::v2::components::banner::BannerVariant::Success,
                },
            );
            cmds.push(Cmd::Fetch(FetchCmd::Workers));
            (model, cmds)
        }
        Err(e) => {
            // Error: show error banner and re-fetch to restore the worker
            let (model, mut cmds) = super::super::update::update(
                model,
                Msg::BannerShow {
                    message: format!("Kill failed: {e}"),
                    variant: crate::v2::components::banner::BannerVariant::Error,
                },
            );
            cmds.push(Cmd::Fetch(FetchCmd::Workers));
            (model, cmds)
        }
    }
}

/// Clamp the selected row to valid bounds after data changes.
pub fn clamp_selection(model: &mut Model) {
    let count = current_page_count(model);
    if count == 0 {
        model.worker_list.selected_row = 0;
    } else if model.worker_list.selected_row >= count {
        model.worker_list.selected_row = count.saturating_sub(1);
    }
}

/// Get the number of workers on the current page.
fn current_page_count(model: &Model) -> usize {
    let total = worker_count(model);
    if total == 0 {
        return 0;
    }
    let start = model.worker_list.current_page * WORKER_PAGE_SIZE;
    if start >= total {
        return 0;
    }
    (start + WORKER_PAGE_SIZE).min(total) - start
}

/// Get the total number of workers.
fn worker_count(model: &Model) -> usize {
    model
        .worker_list
        .data
        .data()
        .map(|d| d.workers.len())
        .unwrap_or(0)
}

/// Input handler for the workers list page.
///
/// Handles navigation (j/k, arrow keys), pagination (h/l),
/// kill (x/X), goto (g), and refresh (r).
pub struct WorkerListHandler;

impl InputHandler for WorkerListHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        match (key.code, key.modifiers) {
            (KeyCode::Char('j'), KeyModifiers::NONE) | (KeyCode::Down, KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::WorkersNavigate { delta: 1 }))
            }
            (KeyCode::Char('k'), KeyModifiers::NONE) | (KeyCode::Up, KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::WorkersNavigate { delta: -1 }))
            }
            (KeyCode::Char('l'), KeyModifiers::NONE) | (KeyCode::Right, KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::WorkersPageRight))
            }
            (KeyCode::Char('h'), KeyModifiers::NONE) | (KeyCode::Left, KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::WorkersPageLeft))
            }
            (KeyCode::Char('r'), KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::WorkersRefresh))
            }
            (KeyCode::Char('x'), KeyModifiers::NONE)
            | (KeyCode::Char('X'), KeyModifiers::SHIFT) => {
                InputResult::Capture(Msg::Nav(NavMsg::WorkersKill))
            }
            (KeyCode::Char('g'), KeyModifiers::NONE) => {
                InputResult::Capture(Msg::Nav(NavMsg::WorkersGoto))
            }
            (KeyCode::Char('s'), KeyModifiers::SHIFT) => {
                InputResult::Capture(Msg::Overlay(OverlayMsg::OpenSettings {
                    custom_theme_names: vec![],
                }))
            }
            _ => InputResult::Bubble,
        }
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "X".to_string(),
                description: "Kill".to_string(),
                common: false,
            },
            FooterCommand {
                key_label: "g".to_string(),
                description: "Goto".to_string(),
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
        "worker_list"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEventKind, KeyEventState};

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

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

    fn model_with_workers(workers: Vec<WorkerSummary>) -> Model {
        let mut model = Model::initial();
        model.worker_list.data = LoadState::Loaded(WorkerListData { workers });
        model
    }

    // ── worker_to_row ─────────────────────────────────────────────────

    #[test]
    fn worker_to_row_fields() {
        let worker = make_worker("ur-abc");
        let row = worker_to_row(&worker);
        assert_eq!(row[0], "ur-abc");
        assert_eq!(row[1], "ur");
        assert_eq!(row[2], "implement");
        assert_eq!(row[3], "running");
        assert_eq!(row[4], "active");
        assert_eq!(row[5], "ctr-abc123");
    }

    // ── sorted_workers ────────────────────────────────────────────────

    #[test]
    fn sorted_workers_orders_by_id() {
        let workers = vec![
            make_worker("ur-def"),
            make_worker("ur-abc"),
            make_worker("ur-ghi"),
        ];
        let sorted = sorted_workers(&workers);
        assert_eq!(sorted[0].worker_id, "ur-abc");
        assert_eq!(sorted[1].worker_id, "ur-def");
        assert_eq!(sorted[2].worker_id, "ur-ghi");
    }

    // ── total_pages ───────────────────────────────────────────────────

    #[test]
    fn total_pages_empty() {
        assert_eq!(total_pages(0), 1);
    }

    #[test]
    fn total_pages_exact() {
        assert_eq!(total_pages(WORKER_PAGE_SIZE), 1);
    }

    #[test]
    fn total_pages_partial() {
        assert_eq!(total_pages(WORKER_PAGE_SIZE + 1), 2);
    }

    // ── navigation ────────────────────────────────────────────────────

    #[test]
    fn navigate_down() {
        let workers: Vec<WorkerSummary> = (0..3).map(|i| make_worker(&format!("ur-{i}"))).collect();
        let mut model = model_with_workers(workers);

        workers_navigate(&mut model, 1);
        assert_eq!(model.worker_list.selected_row, 1);

        workers_navigate(&mut model, 1);
        assert_eq!(model.worker_list.selected_row, 2);

        // At bottom, stays at 2
        workers_navigate(&mut model, 1);
        assert_eq!(model.worker_list.selected_row, 2);
    }

    #[test]
    fn navigate_up() {
        let workers: Vec<WorkerSummary> = (0..3).map(|i| make_worker(&format!("ur-{i}"))).collect();
        let mut model = model_with_workers(workers);
        model.worker_list.selected_row = 2;

        workers_navigate(&mut model, -1);
        assert_eq!(model.worker_list.selected_row, 1);

        workers_navigate(&mut model, -1);
        assert_eq!(model.worker_list.selected_row, 0);

        // At top, stays at 0
        workers_navigate(&mut model, -1);
        assert_eq!(model.worker_list.selected_row, 0);
    }

    #[test]
    fn navigate_empty_is_noop() {
        let mut model = model_with_workers(vec![]);
        workers_navigate(&mut model, 1);
        assert_eq!(model.worker_list.selected_row, 0);
    }

    // ── pagination ────────────────────────────────────────────────────

    #[test]
    fn page_right_advances() {
        let workers: Vec<WorkerSummary> = (0..45)
            .map(|i| make_worker(&format!("ur-{i:02}")))
            .collect();
        let mut model = model_with_workers(workers);

        assert_eq!(model.worker_list.current_page, 0);
        workers_page_right(&mut model);
        assert_eq!(model.worker_list.current_page, 1);
        assert_eq!(model.worker_list.selected_row, 0);

        workers_page_right(&mut model);
        assert_eq!(model.worker_list.current_page, 2);

        // Can't go past last page
        workers_page_right(&mut model);
        assert_eq!(model.worker_list.current_page, 2);
    }

    #[test]
    fn page_left_goes_back() {
        let workers: Vec<WorkerSummary> = (0..45)
            .map(|i| make_worker(&format!("ur-{i:02}")))
            .collect();
        let mut model = model_with_workers(workers);
        model.worker_list.current_page = 2;

        workers_page_left(&mut model);
        assert_eq!(model.worker_list.current_page, 1);

        workers_page_left(&mut model);
        assert_eq!(model.worker_list.current_page, 0);

        // Can't go before first page
        workers_page_left(&mut model);
        assert_eq!(model.worker_list.current_page, 0);
    }

    // ── selected_worker_id ────────────────────────────────────────────

    #[test]
    fn selected_worker_id_returns_sorted() {
        let workers = vec![
            make_worker("ur-def"),
            make_worker("ur-abc"),
            make_worker("ur-ghi"),
        ];
        let model = model_with_workers(workers);
        // First in sorted order is ur-abc
        assert_eq!(selected_worker_id(&model), Some("ur-abc".to_string()));
    }

    #[test]
    fn selected_worker_id_empty() {
        let model = model_with_workers(vec![]);
        assert_eq!(selected_worker_id(&model), None);
    }

    #[test]
    fn selected_worker_id_not_loaded() {
        let model = Model::initial();
        assert_eq!(selected_worker_id(&model), None);
    }

    // ── handle_kill ───────────────────────────────────────────────────

    #[test]
    fn handle_kill_removes_worker_and_returns_cmd() {
        let workers = vec![make_worker("ur-abc"), make_worker("ur-def")];
        let mut model = model_with_workers(workers);

        let cmd = handle_kill(&mut model);
        assert!(matches!(cmd, Cmd::StopWorker { .. }));

        // Worker should be removed from data
        let data = model.worker_list.data.data().unwrap();
        assert_eq!(data.workers.len(), 1);
        assert!(!data.workers.iter().any(|w| w.worker_id == "ur-abc"));
    }

    #[test]
    fn handle_kill_empty_returns_none() {
        let mut model = model_with_workers(vec![]);
        let cmd = handle_kill(&mut model);
        assert!(matches!(cmd, Cmd::None));
    }

    // ── handle_goto ───────────────────────────────────────────────────

    #[test]
    fn handle_goto_opens_overlay() {
        let workers = vec![make_worker("ur-abc")];
        let mut model = model_with_workers(workers);

        handle_goto(&mut model);
        assert!(model.active_overlay.is_some());
    }

    #[test]
    fn handle_goto_empty_is_noop() {
        let mut model = model_with_workers(vec![]);
        handle_goto(&mut model);
        assert!(model.active_overlay.is_none());
    }

    // ── clamp_selection ───────────────────────────────────────────────

    #[test]
    fn clamp_selection_on_empty() {
        let mut model = model_with_workers(vec![]);
        model.worker_list.selected_row = 5;
        clamp_selection(&mut model);
        assert_eq!(model.worker_list.selected_row, 0);
    }

    #[test]
    fn clamp_selection_within_bounds() {
        let workers = vec![make_worker("ur-abc"), make_worker("ur-def")];
        let mut model = model_with_workers(workers);
        model.worker_list.selected_row = 1;
        clamp_selection(&mut model);
        assert_eq!(model.worker_list.selected_row, 1);
    }

    #[test]
    fn clamp_selection_exceeds_bounds() {
        let workers = vec![make_worker("ur-abc")];
        let mut model = model_with_workers(workers);
        model.worker_list.selected_row = 5;
        clamp_selection(&mut model);
        assert_eq!(model.worker_list.selected_row, 0);
    }

    // ── input handler ─────────────────────────────────────────────────

    #[test]
    fn handler_j_captures_navigate_down() {
        let handler = WorkerListHandler;
        let key = make_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_k_captures_navigate_up() {
        let handler = WorkerListHandler;
        let key = make_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_x_captures_kill() {
        let handler = WorkerListHandler;
        let key = make_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_g_captures_goto() {
        let handler = WorkerListHandler;
        let key = make_key(KeyCode::Char('g'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_r_captures_refresh() {
        let handler = WorkerListHandler;
        let key = make_key(KeyCode::Char('r'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Capture(_)));
    }

    #[test]
    fn handler_unknown_bubbles() {
        let handler = WorkerListHandler;
        let key = make_key(KeyCode::Char('z'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn handler_footer_has_kill() {
        let handler = WorkerListHandler;
        let commands = handler.footer_commands();
        assert!(commands.iter().any(|c| c.description == "Kill"));
    }

    #[test]
    fn handler_footer_has_goto() {
        let handler = WorkerListHandler;
        let commands = handler.footer_commands();
        assert!(commands.iter().any(|c| c.description == "Goto"));
    }

    #[test]
    fn handler_name() {
        let handler = WorkerListHandler;
        assert_eq!(handler.name(), "worker_list");
    }

    // ── handle_workers_nav integration ────────────────────────────────

    #[test]
    fn handle_nav_navigate() {
        let workers: Vec<WorkerSummary> = (0..3).map(|i| make_worker(&format!("ur-{i}"))).collect();
        let model = model_with_workers(workers);

        let (new_model, cmds) = handle_workers_nav(model, NavMsg::WorkersNavigate { delta: 1 });
        assert_eq!(new_model.worker_list.selected_row, 1);
        assert!(cmds.is_empty());
    }

    #[test]
    fn handle_nav_refresh() {
        let workers = vec![make_worker("ur-abc")];
        let model = model_with_workers(workers);

        let (new_model, cmds) = handle_workers_nav(model, NavMsg::WorkersRefresh);
        assert!(new_model.worker_list.data.is_loading());
        assert!(!cmds.is_empty());
    }

    #[test]
    fn handle_nav_kill() {
        let workers = vec![make_worker("ur-abc")];
        let model = model_with_workers(workers);

        let (new_model, cmds) = handle_workers_nav(model, NavMsg::WorkersKill);
        // Worker should be optimistically removed
        let data = new_model.worker_list.data.data().unwrap();
        assert!(data.workers.is_empty());
        assert_eq!(cmds.len(), 1);
        assert!(matches!(cmds[0], Cmd::StopWorker { .. }));
    }
}
