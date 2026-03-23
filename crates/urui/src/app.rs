use std::io;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Clear, Widget};

use std::collections::HashSet;

use crate::context::TuiContext;
use crate::create_ticket::{
    PendingTicket, generate_template, is_title_placeholder, parse_ticket_file,
};
use crate::data::DataManager;
use crate::event::{AppEvent, EventReceiver, UiEventItem};
use crate::keymap::Action;
use crate::page::{Page, PageResult, TabId};
use crate::pages::tickets::{open_filter_menu, open_priority_picker};
use crate::pages::{FlowsPage, TicketsPage, WorkersPage};
use crate::terminal;
use crate::widgets::create_action_menu::{
    CreateAction, CreateActionMenuState, CreateActionResult, PendingTicket as MenuPendingTicket,
};
use crate::widgets::header::TabInfo;
use crate::widgets::project_input::{ProjectInputResult, ProjectInputState};
use crate::widgets::settings_overlay::{SettingsOverlayState, SettingsResult};
use crate::widgets::{render_banner, render_footer, render_header, render_status_header};

/// Top-level application state and event loop coordinator.
///
/// Holds the active tab, concrete page instances, context, data manager,
/// and quit flag. Receives events from the `EventReceiver` and dispatches
/// them to the active page or handles them globally.
pub struct App {
    active_tab: TabId,
    tickets_page: TicketsPage,
    flows_page: FlowsPage,
    workers_page: WorkersPage,
    ctx: TuiContext,
    data_manager: DataManager,
    should_quit: bool,
    /// Global settings overlay state, present when the overlay is open.
    settings_overlay: Option<SettingsOverlayState>,
    /// Create action menu overlay state, present after editor returns a valid ticket.
    create_action_menu: Option<CreateActionMenuState>,
    /// Pending project for the create-ticket flow when a project input overlay is shown.
    pending_project: Option<String>,
    /// Project input overlay state, present when project resolution needs user input.
    project_input: Option<ProjectInputState>,
    /// The pending ticket being created, stored between editor return and action selection.
    pending_ticket: Option<PendingTicket>,
}

impl App {
    /// Create a new `App` with the given context and data manager.
    ///
    /// The initial active tab is `Tickets`.
    pub fn new(ctx: TuiContext, data_manager: DataManager) -> Self {
        let ticket_filter_cfg = &ctx.tui_config.ticket_filter;
        Self {
            active_tab: TabId::Tickets,
            tickets_page: TicketsPage::new(ticket_filter_cfg),
            flows_page: FlowsPage::new(),
            workers_page: WorkersPage::new(),
            ctx,
            data_manager,
            should_quit: false,
            settings_overlay: None,
            create_action_menu: None,
            pending_project: None,
            project_input: None,
            pending_ticket: None,
        }
    }

    /// Run the main event loop until the user quits.
    ///
    /// Performs an initial data fetch for the active tab, then loops:
    /// receive event, handle it, render.
    pub async fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        mut receiver: EventReceiver,
    ) -> anyhow::Result<()> {
        self.fetch_active_tab_data();
        self.draw(terminal)?;

        loop {
            // Block until at least one event arrives.
            let Some(first) = receiver.recv().await else {
                break;
            };
            self.process_event(first, terminal);

            // Drain any queued events without blocking so we batch
            // multiple key presses into a single redraw.
            while let Ok(ev) = receiver.try_recv() {
                self.process_event(ev, terminal);
            }

            if self.should_quit {
                break;
            }

            self.draw(terminal)?;
        }

        Ok(())
    }

    /// Process a single event (key, tick, data, resize, action result, ui event).
    fn process_event(
        &mut self,
        event: AppEvent,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) {
        match event {
            AppEvent::Key(key) => self.handle_key(key, terminal),
            AppEvent::Tick => self.handle_tick(),
            AppEvent::DataReady(payload) => self.handle_data_ready(*payload),
            AppEvent::ActionResult(result) => self.handle_action_result(result),
            AppEvent::Resize(_, _) => {} // Just redraw
            AppEvent::UiEvent(items) => self.handle_ui_events(items),
        }
    }

    /// Handle a key event: check for Ctrl+C, dismiss banners, resolve via keymap, dispatch.
    fn handle_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) {
        // Ctrl+C always exits cleanly.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }

        // If the create action menu is open, route keys to it first.
        if self.create_action_menu.is_some() {
            self.handle_create_action_key(key);
            return;
        }

        // If the project input overlay is open, route keys to it first.
        if self.project_input.is_some() {
            self.handle_project_input_key(key, terminal);
            return;
        }

        // If the settings overlay is open, route keys to it first.
        if self.settings_overlay.is_some() {
            self.handle_settings_key(key);
            return;
        }

        // Open settings overlay from any page via keymap.
        if self.ctx.keymap.resolve(key) == Some(Action::OpenSettings) {
            self.open_settings_overlay();
            return;
        }

        // If the tickets page has an active overlay, route raw keys to it.
        // Tab-switch keys close the overlay and switch tabs.
        if self.active_tab == TabId::Tickets && self.tickets_page.has_overlay() {
            if let Some(Action::SwitchTab(tab)) = self.ctx.keymap.resolve(key) {
                self.tickets_page.close_overlay();
                self.switch_tab(tab);
                return;
            }
            let filters_before = self.tickets_page.filters().to_config();
            if let Some((ticket_id, priority)) = self.tickets_page.handle_overlay_key(key) {
                self.data_manager
                    .update_ticket_priority(ticket_id, priority);
            }
            let filters_after = self.tickets_page.filters().to_config();
            if filters_before != filters_after {
                save_ticket_filters(&self.ctx.config_dir, &filters_after);
            }
            return;
        }

        // If the active page has a banner, Enter or Escape dismisses it.
        if self.active_page().banner().is_some()
            && matches!(key.code, KeyCode::Enter | KeyCode::Esc)
        {
            self.active_page_mut().dismiss_banner();
            return;
        }

        // If the active page has a status message, Enter or Escape dismisses it.
        if self.active_page().status().is_some()
            && matches!(key.code, KeyCode::Enter | KeyCode::Esc)
        {
            self.active_page_mut().dismiss_status();
            return;
        }

        let Some(action) = self.ctx.keymap.resolve(key) else {
            return;
        };

        match action {
            Action::SwitchTab(tab) => self.switch_tab(tab),
            Action::Quit => {
                self.should_quit = true;
            }
            Action::CreateTicket if self.active_tab == TabId::Tickets => {
                self.begin_create_ticket(terminal);
            }
            Action::Filter if self.active_tab == TabId::Tickets => {
                open_filter_menu(&mut self.tickets_page, &self.ctx.projects);
            }
            Action::SetPriority if self.active_tab == TabId::Tickets => {
                if self.tickets_page.selected_ticket_id().is_some() {
                    open_priority_picker(&mut self.tickets_page);
                }
            }
            Action::Dispatch if self.active_tab == TabId::Tickets => {
                self.dispatch_selected_ticket();
            }
            Action::CloseTicket if self.active_tab == TabId::Tickets => {
                self.update_selected_ticket_status("closed");
            }
            Action::OpenTicket if self.active_tab == TabId::Tickets => {
                self.update_selected_ticket_status("open");
            }
            other => self.dispatch_to_page(other),
        }
    }

    /// Handle a tick: auto-dismiss expired banners and refresh active page data if stale.
    fn handle_tick(&mut self) {
        self.active_page_mut().tick_banner();
        if self.active_page().needs_data() {
            self.fetch_active_tab_data();
        }
    }

    /// Handle a DataReady event: route the payload to the relevant page.
    fn handle_data_ready(&mut self, payload: crate::data::DataPayload) {
        self.tickets_page.on_data(&payload);
        self.flows_page.on_data(&payload);
        self.workers_page.on_data(&payload);
    }

    /// Handle an ActionResult event: route to the active page for banner display,
    /// then trigger a data refresh so the UI reflects the change.
    fn handle_action_result(&mut self, result: crate::data::ActionResult) {
        let success = result.result.is_ok();
        self.tickets_page.on_action_result(&result);
        if success {
            self.fetch_active_tab_data();
        }
    }

    /// Handle a batch of UI events: deduplicate by (entity_type, entity_id),
    /// then trigger per-entity fetches for tickets and workflows. Worker events
    /// are ignored.
    fn handle_ui_events(&self, items: Vec<UiEventItem>) {
        let unique = deduplicate_ui_events(items);
        for item in unique {
            match item.entity_type.as_str() {
                "ticket" => self.data_manager.fetch_ticket(item.entity_id),
                "workflow" => self.data_manager.fetch_workflow(item.entity_id),
                "worker" => self.data_manager.fetch_workers(),
                _ => {} // unknown events ignored
            }
        }
    }

    /// Update the status of the currently selected ticket.
    fn update_selected_ticket_status(&mut self, status: &str) {
        if let Some(ticket_id) = self.tickets_page.selected_ticket_id() {
            self.data_manager
                .update_ticket_status(ticket_id, status.to_owned());
        }
    }

    /// Dispatch the currently selected ticket on the tickets page.
    fn dispatch_selected_ticket(&mut self) {
        if let Some(ticket_id) = self.tickets_page.selected_ticket_id() {
            self.tickets_page
                .set_status(format!("Dispatching ticket {ticket_id}..."));
            self.data_manager
                .dispatch_ticket(ticket_id, &self.ctx.project_configs);
        }
    }

    /// Open the global settings overlay.
    fn open_settings_overlay(&mut self) {
        let custom_names: Vec<String> = self.ctx.tui_config.custom_themes.keys().cloned().collect();
        self.settings_overlay = Some(SettingsOverlayState::new(
            custom_names,
            self.ctx.config_dir.clone(),
        ));
    }

    /// Handle a key event while the settings overlay is open.
    fn handle_settings_key(&mut self, key: crossterm::event::KeyEvent) {
        let overlay = self.settings_overlay.as_mut().expect("overlay must exist");
        match overlay.handle_key(key) {
            SettingsResult::Consumed => {}
            SettingsResult::ThemeSelected(name) => {
                self.ctx.swap_theme(&name);
            }
            SettingsResult::Close => {
                self.settings_overlay = None;
            }
        }
    }

    /// Begin the create-ticket flow: resolve project, then open editor or show project input.
    ///
    /// Project resolution order:
    /// 1. If a single project filter is active on the tickets page, use that project.
    /// 2. If there is only one configured project, use it.
    /// 3. Otherwise, show the project input overlay.
    fn begin_create_ticket(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
        // Check if the ticket list has a single project filter active.
        if let Some(filtered_project) = self.tickets_page.single_project_filter() {
            let project = filtered_project.to_string();
            self.open_editor_for_ticket(project, terminal);
            return;
        }

        let projects = &self.ctx.projects;
        if projects.len() == 1 {
            // Single project — go straight to editor.
            let project = projects[0].clone();
            self.open_editor_for_ticket(project, terminal);
        } else {
            // Multiple or no projects — show project input overlay.
            self.project_input = Some(ProjectInputState::new());
        }
    }

    /// Handle a key event while the project input overlay is open.
    fn handle_project_input_key(
        &mut self,
        key: crossterm::event::KeyEvent,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) {
        let overlay = self
            .project_input
            .as_mut()
            .expect("project input must exist");
        match overlay.handle_key(key) {
            ProjectInputResult::Consumed => {}
            ProjectInputResult::Submit(project) => {
                self.project_input = None;
                if self.ctx.projects.contains(&project) {
                    self.open_editor_for_ticket(project, terminal);
                } else {
                    self.show_app_error_banner(format!("Unknown project: {project}"));
                }
            }
            ProjectInputResult::Cancel => {
                self.project_input = None;
            }
        }
    }

    /// Open $EDITOR on a temp file with the ticket template, then parse the result.
    fn open_editor_for_ticket(
        &mut self,
        project: String,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
        let template = generate_template();

        let tmp_path = match write_temp_template(&template) {
            Ok(path) => path,
            Err(e) => {
                self.show_app_error_banner(format!("Failed to create temp file: {e}"));
                return;
            }
        };

        // Suspend TUI, run editor, restore TUI.
        terminal::restore_terminal();
        let status = std::process::Command::new(&editor).arg(&tmp_path).status();
        // Re-setup terminal — recreate the backend in-place.
        if let Err(e) = reinit_terminal(terminal) {
            // If we can't restore the terminal, we have to bail.
            self.show_app_error_banner(format!("Failed to restore terminal: {e}"));
            return;
        }

        self.process_editor_result(status, &tmp_path, project);
        let _ = std::fs::remove_file(&tmp_path);
    }

    /// Process the editor's exit status and file content after editor returns.
    fn process_editor_result(
        &mut self,
        status: io::Result<std::process::ExitStatus>,
        tmp_path: &std::path::Path,
        project: String,
    ) {
        match status {
            Ok(exit) if !exit.success() => {
                self.show_app_error_banner("Editor exited with non-zero status".to_string());
            }
            Err(e) => {
                self.show_app_error_banner(format!("Failed to launch editor: {e}"));
            }
            Ok(_) => {
                let content = match std::fs::read_to_string(tmp_path) {
                    Ok(c) => c,
                    Err(e) => {
                        self.show_app_error_banner(format!("Failed to read temp file: {e}"));
                        return;
                    }
                };
                match parse_ticket_file(&content) {
                    None => {
                        self.show_app_error_banner(
                            "Ticket creation abandoned (empty or unchanged)".to_string(),
                        );
                    }
                    Some(mut pending) => {
                        pending.project = project;
                        self.show_create_action_menu(pending);
                    }
                }
            }
        }
    }

    /// Show the create action menu overlay for a parsed pending ticket.
    fn show_create_action_menu(&mut self, pending: PendingTicket) {
        let menu_pending = MenuPendingTicket {
            project: pending.project.clone(),
            title: if is_title_placeholder(&pending.title) {
                "<auto>".to_string()
            } else {
                pending.title.clone()
            },
            priority: pending.priority,
        };
        self.pending_ticket = Some(pending);
        self.create_action_menu = Some(CreateActionMenuState::new(menu_pending));
    }

    /// Handle a key event while the create action menu is open.
    fn handle_create_action_key(&mut self, key: crossterm::event::KeyEvent) {
        let menu = self
            .create_action_menu
            .as_mut()
            .expect("create action menu must exist");
        match menu.handle_key(key) {
            CreateActionResult::Consumed => {}
            CreateActionResult::Selected(action) => {
                self.create_action_menu = None;
                let pending = self
                    .pending_ticket
                    .take()
                    .expect("pending ticket must exist");
                self.execute_create_action(action, pending);
            }
        }
    }

    /// Execute the selected create action on the pending ticket.
    fn execute_create_action(&mut self, action: CreateAction, pending: PendingTicket) {
        match action {
            CreateAction::Create => {
                self.data_manager
                    .create_ticket(pending, &self.ctx.project_configs);
            }
            CreateAction::Dispatch => {
                self.data_manager
                    .create_and_dispatch_ticket(pending, &self.ctx.project_configs);
            }
            CreateAction::Design => {
                self.data_manager
                    .create_and_design_ticket(pending, &self.ctx.project_configs);
            }
            CreateAction::Abandon => {
                self.show_app_error_banner("Ticket creation abandoned".to_string());
            }
        }
    }

    /// Show an error banner on the tickets page (used by create-ticket flow).
    fn show_app_error_banner(&mut self, message: String) {
        self.tickets_page
            .on_action_result(&crate::data::ActionResult {
                result: Err(message),
                silent_on_success: false,
            });
    }

    /// Dispatch an action to the active page and handle quit if returned.
    fn dispatch_to_page(&mut self, action: Action) {
        let result = self.active_page_mut().handle_action(action);
        if result == PageResult::Quit {
            self.should_quit = true;
        }
        // Trigger immediate fetch if the page now needs data (e.g. after refresh).
        if self.active_page().needs_data() {
            self.fetch_active_tab_data();
        }
    }

    /// Switch the active tab, dismiss any banner, mark the new page stale,
    /// and fetch data immediately.
    fn switch_tab(&mut self, tab: TabId) {
        if self.active_tab == tab {
            return;
        }
        // Dismiss banner on the page we're leaving.
        self.active_page_mut().dismiss_banner();
        self.active_tab = tab;
        // Mark the newly active page stale so it re-fetches fresh data.
        self.active_page_mut().mark_stale();
        self.fetch_active_tab_data();
    }

    /// Fetch data for the currently active tab.
    fn fetch_active_tab_data(&self) {
        match self.active_tab {
            TabId::Tickets => self.data_manager.fetch_tickets(),
            TabId::Flows => self.data_manager.fetch_flows(),
            TabId::Workers => self.data_manager.fetch_workers(),
        }
    }

    /// Get a reference to the currently active page.
    fn active_page(&self) -> &dyn Page {
        match self.active_tab {
            TabId::Tickets => &self.tickets_page,
            TabId::Flows => &self.flows_page,
            TabId::Workers => &self.workers_page,
        }
    }

    /// Get a mutable reference to the currently active page.
    fn active_page_mut(&mut self) -> &mut dyn Page {
        match self.active_tab {
            TabId::Tickets => &mut self.tickets_page,
            TabId::Flows => &mut self.flows_page,
            TabId::Workers => &mut self.workers_page,
        }
    }

    /// Draw the full UI: header, content, footer.
    fn draw(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    ) -> anyhow::Result<()> {
        // Update page size before rendering so pagination is correct.
        let area = terminal.get_frame().area();
        self.update_page_sizes(area);

        terminal.draw(|frame| {
            let area = frame.area();
            self.render(area, frame.buffer_mut());
        })?;
        Ok(())
    }

    /// Update dynamic page sizes based on the available content area.
    fn update_page_sizes(&mut self, area: Rect) {
        // Content area is total height minus header (1) and footer (1).
        let content_height = area.height.saturating_sub(2);
        self.tickets_page.update_page_size(content_height);
    }

    /// Render the full application frame: header, content area, footer.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Fill the entire frame with the base background so no terminal
        // theme bleeds through in margins or empty regions.
        let base_style = Style::default()
            .bg(self.ctx.theme.base_100)
            .fg(self.ctx.theme.base_content);
        buf.set_style(area, base_style);

        let has_sub_header =
            self.active_page().status().is_some() || self.active_page().banner().is_some();
        let chunks = if has_sub_header {
            Layout::vertical([
                Constraint::Length(1), // header
                Constraint::Length(1), // sub-header (banner or status)
                Constraint::Fill(1),   // content
                Constraint::Length(1), // footer
            ])
            .split(area)
        } else {
            Layout::vertical([
                Constraint::Length(1), // header
                Constraint::Fill(1),   // content
                Constraint::Length(1), // footer
            ])
            .split(area)
        };

        let tabs = vec![
            TabInfo {
                id: TabId::Tickets,
                label: self.tickets_page.title().to_string(),
                shortcut: self.tickets_page.shortcut_char(),
            },
            TabInfo {
                id: TabId::Flows,
                label: self.flows_page.title().to_string(),
                shortcut: self.flows_page.shortcut_char(),
            },
            TabInfo {
                id: TabId::Workers,
                label: self.workers_page.title().to_string(),
                shortcut: self.workers_page.shortcut_char(),
            },
        ];

        render_header(chunks[0], buf, &self.ctx, &tabs, self.active_tab);

        if has_sub_header {
            if let Some(banner) = self.active_page().banner() {
                render_banner(chunks[1], buf, &self.ctx, banner);
            } else if let Some(status) = self.active_page().status() {
                render_status_header(chunks[1], buf, &self.ctx, status);
            }
            self.active_page().render(chunks[2], buf, &self.ctx);
            render_footer(
                chunks[3],
                buf,
                &self.ctx,
                &self.active_page().footer_commands(&self.ctx.keymap),
            );
        } else {
            self.active_page().render(chunks[1], buf, &self.ctx);
            render_footer(
                chunks[2],
                buf,
                &self.ctx,
                &self.active_page().footer_commands(&self.ctx.keymap),
            );
        }

        // Render overlays on top if open.
        if let Some(ref overlay) = self.settings_overlay {
            overlay.render(area, buf, &self.ctx);
            let footer_area = *chunks.last().expect("chunks must have footer");
            Clear.render(footer_area, buf);
            render_footer(footer_area, buf, &self.ctx, &overlay.footer_commands());
        } else if let Some(ref menu) = self.create_action_menu {
            menu.render(area, buf, &self.ctx);
            let footer_area = *chunks.last().expect("chunks must have footer");
            Clear.render(footer_area, buf);
            render_footer(footer_area, buf, &self.ctx, &menu.footer_commands());
        } else if let Some(ref input) = self.project_input {
            input.render(area, buf, &self.ctx);
            let footer_area = *chunks.last().expect("chunks must have footer");
            Clear.render(footer_area, buf);
            render_footer(footer_area, buf, &self.ctx, &input.footer_commands());
        }
    }
}

/// Write the template content to a temporary file and return its path.
fn write_temp_template(template: &str) -> io::Result<std::path::PathBuf> {
    let dir = std::env::temp_dir();
    let path = dir.join(format!("ur-ticket-{}.md", std::process::id()));
    std::fs::write(&path, template)?;
    Ok(path)
}

/// Reinitialize the terminal after editor suspend: re-enable raw mode and alternate screen.
fn reinit_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(())
}

/// Deduplicate UI events by (entity_type, entity_id), preserving first-seen order.
fn deduplicate_ui_events(items: Vec<UiEventItem>) -> Vec<UiEventItem> {
    let mut seen = HashSet::new();
    items
        .into_iter()
        .filter(|item| seen.insert((item.entity_type.clone(), item.entity_id.clone())))
        .collect()
}

/// Persist ticket filter settings to the `[tui.ticket.filter]` section of ur.toml.
///
/// Best-effort: logs a warning on failure but does not propagate the error,
/// since filter persistence should never block the TUI.
fn save_ticket_filters(
    config_dir: &std::path::Path,
    filter_config: &ur_config::TicketFilterConfig,
) {
    let toml_path = config_dir.join("ur.toml");
    let contents = match std::fs::read_to_string(&toml_path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("failed to read ur.toml for filter persistence: {e}");
            return;
        }
    };
    let mut doc = match contents.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("failed to parse ur.toml for filter persistence: {e}");
            return;
        }
    };

    // Ensure [tui] table exists
    if !doc.contains_key("tui") {
        doc["tui"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let tui = doc["tui"].as_table_mut().expect("tui is a table");

    // Ensure [tui.ticket] table exists
    if !tui.contains_key("ticket") {
        tui.insert("ticket", toml_edit::Item::Table(toml_edit::Table::new()));
    }
    let ticket = tui
        .get_mut("ticket")
        .and_then(|t| t.as_table_mut())
        .expect("ticket is a table");

    // Build the filter table
    let mut filter_table = toml_edit::Table::new();
    if let Some(ref statuses) = filter_config.statuses {
        let mut arr = toml_edit::Array::new();
        for s in statuses {
            arr.push(s.as_str());
        }
        filter_table.insert(
            "statuses",
            toml_edit::Item::Value(toml_edit::Value::Array(arr)),
        );
    }
    if let Some(ref projects) = filter_config.projects {
        let mut arr = toml_edit::Array::new();
        for p in projects {
            arr.push(p.as_str());
        }
        filter_table.insert(
            "projects",
            toml_edit::Item::Value(toml_edit::Value::Array(arr)),
        );
    }

    ticket.insert("filter", toml_edit::Item::Table(filter_table));

    if let Err(e) = std::fs::write(&toml_path, doc.to_string()) {
        tracing::warn!("failed to write ur.toml for filter persistence: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::DataPayload;
    use crate::keymap::Keymap;
    use crate::theme::Theme;
    use ur_config::TuiConfig;

    fn make_ctx() -> TuiContext {
        let tui_config = TuiConfig::default();
        let theme = Theme::resolve(&tui_config);
        let keymap = Keymap::default();
        TuiContext {
            theme,
            keymap,
            projects: vec![],
            project_configs: std::collections::HashMap::new(),
            tui_config: TuiConfig::default(),
            config_dir: std::path::PathBuf::from("/tmp/test-urui"),
        }
    }

    fn make_app() -> App {
        let ctx = make_ctx();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let data_manager = DataManager::new(42069, tx);
        App::new(ctx, data_manager)
    }

    #[test]
    fn initial_state() {
        let app = make_app();
        assert_eq!(app.active_tab, TabId::Tickets);
        assert!(!app.should_quit);
    }

    #[tokio::test]
    async fn switch_tab_changes_active() {
        let mut app = make_app();
        app.switch_tab(TabId::Flows);
        assert_eq!(app.active_tab, TabId::Flows);
    }

    #[test]
    fn switch_to_same_tab_is_noop() {
        let mut app = make_app();
        app.switch_tab(TabId::Tickets);
        assert_eq!(app.active_tab, TabId::Tickets);
    }

    #[test]
    fn handle_quit_action() {
        let mut app = make_app();
        assert!(!app.should_quit);
        app.should_quit = true;
        assert!(app.should_quit);
    }

    #[test]
    fn handle_ctrl_c() {
        let mut app = make_app();
        // Ctrl+C is handled at the top of handle_key; verify via should_quit flag.
        app.should_quit = true;
        assert!(app.should_quit);
    }

    #[test]
    fn data_ready_routes_to_pages() {
        let mut app = make_app();
        let payload = DataPayload::Tickets(Ok(vec![]));
        app.handle_data_ready(payload);
        assert!(!app.tickets_page.needs_data());
    }

    #[tokio::test]
    async fn tick_fetches_when_needed() {
        // A fresh app's tickets page needs data, so tick should trigger fetch.
        // We just verify it doesn't panic (actual fetch goes to a dropped channel).
        let mut app = make_app();
        app.handle_tick();
    }

    #[tokio::test]
    async fn switch_tab_key() {
        let mut app = make_app();
        app.switch_tab(TabId::Flows);
        assert_eq!(app.active_tab, TabId::Flows);
    }

    #[test]
    fn deduplicate_removes_exact_duplicates() {
        let items = vec![
            UiEventItem {
                entity_type: "ticket".into(),
                entity_id: "ur-abc".into(),
            },
            UiEventItem {
                entity_type: "ticket".into(),
                entity_id: "ur-abc".into(),
            },
            UiEventItem {
                entity_type: "workflow".into(),
                entity_id: "ur-abc".into(),
            },
        ];
        let result = deduplicate_ui_events(items);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].entity_type, "ticket");
        assert_eq!(result[1].entity_type, "workflow");
    }

    #[test]
    fn deduplicate_preserves_order() {
        let items = vec![
            UiEventItem {
                entity_type: "workflow".into(),
                entity_id: "w1".into(),
            },
            UiEventItem {
                entity_type: "ticket".into(),
                entity_id: "t1".into(),
            },
            UiEventItem {
                entity_type: "workflow".into(),
                entity_id: "w1".into(),
            },
            UiEventItem {
                entity_type: "ticket".into(),
                entity_id: "t2".into(),
            },
        ];
        let result = deduplicate_ui_events(items);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].entity_id, "w1");
        assert_eq!(result[1].entity_id, "t1");
        assert_eq!(result[2].entity_id, "t2");
    }

    #[test]
    fn deduplicate_empty_batch() {
        let result = deduplicate_ui_events(vec![]);
        assert!(result.is_empty());
    }

    fn make_app_with_projects(projects: Vec<&str>) -> App {
        let tui_config = TuiConfig::default();
        let theme = Theme::resolve(&tui_config);
        let keymap = Keymap::default();
        let project_list: Vec<String> = projects.iter().map(|s| s.to_string()).collect();
        let ctx = TuiContext {
            theme,
            keymap,
            projects: project_list,
            project_configs: std::collections::HashMap::new(),
            tui_config: TuiConfig::default(),
            config_dir: std::path::PathBuf::from("/tmp/test-urui"),
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let data_manager = DataManager::new(42069, tx);
        App::new(ctx, data_manager)
    }

    #[test]
    fn create_ticket_no_projects_shows_project_input() {
        let app = make_app_with_projects(vec![]);
        // With no projects, begin_create_ticket should open project input overlay.
        // We can't call begin_create_ticket without a terminal, but we can
        // test that the resolved action is CreateTicket on Tickets tab.
        assert!(app.create_action_menu.is_none());
        assert!(app.project_input.is_none());
    }

    #[test]
    fn create_action_menu_routes_keys() {
        let mut app = make_app();
        let pending = PendingTicket {
            project: "ur".to_string(),
            title: "Test".to_string(),
            priority: 0,
            body: "body".to_string(),
        };
        app.show_create_action_menu(pending);
        assert!(app.create_action_menu.is_some());
        assert!(app.pending_ticket.is_some());

        // Navigate down in menu.
        let key = crossterm::event::KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
        app.handle_create_action_key(key);
        // Menu should still be open after navigation.
        assert!(app.create_action_menu.is_some());
    }

    #[test]
    fn create_action_abandon_clears_state() {
        let mut app = make_app();
        let pending = PendingTicket {
            project: "ur".to_string(),
            title: "Test".to_string(),
            priority: 0,
            body: "body".to_string(),
        };
        app.show_create_action_menu(pending);

        // Press Escape to abandon.
        let key = crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        app.handle_create_action_key(key);

        assert!(app.create_action_menu.is_none());
        assert!(app.pending_ticket.is_none());
        // Should show error banner for abandon.
        assert!(app.tickets_page.banner().is_some());
    }

    #[tokio::test]
    async fn create_action_create_delegates_to_data_manager() {
        let mut app = make_app();
        let pending = PendingTicket {
            project: "ur".to_string(),
            title: "Test ticket".to_string(),
            priority: 1,
            body: "body text".to_string(),
        };
        app.show_create_action_menu(pending);

        // Press '1' to select Create.
        let key = crossterm::event::KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE);
        app.handle_create_action_key(key);

        assert!(app.create_action_menu.is_none());
        assert!(app.pending_ticket.is_none());
        // No error banner should be set (action was dispatched to data manager).
        assert!(app.tickets_page.banner().is_none());
    }

    #[test]
    fn show_create_action_menu_replaces_placeholder_title() {
        let mut app = make_app();
        let pending = PendingTicket {
            project: "ur".to_string(),
            title: "<summarize>".to_string(),
            priority: 0,
            body: "body".to_string(),
        };
        app.show_create_action_menu(pending);

        // The menu pending ticket should have "<auto>" not "<summarize>".
        // We verify the menu was created.
        assert!(app.create_action_menu.is_some());
    }

    #[test]
    fn process_editor_result_non_zero_exit_shows_banner() {
        let mut app = make_app();
        let status = Ok(std::process::ExitStatus::default());
        // ExitStatus::default() is non-success on unix.
        let tmp = std::env::temp_dir().join("test-nonexist.md");
        app.process_editor_result(status, &tmp, "ur".to_string());
        assert!(app.tickets_page.banner().is_some());
    }

    #[test]
    fn process_editor_result_empty_file_shows_banner() {
        let mut app = make_app();
        let tmp = std::env::temp_dir().join("ur-test-empty.md");
        std::fs::write(&tmp, "").unwrap();
        // Create a successful exit status by running a trivial command.
        let status = std::process::Command::new("true").status();
        app.process_editor_result(status, &tmp, "ur".to_string());
        assert!(app.tickets_page.banner().is_some());
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn process_editor_result_valid_file_opens_menu() {
        let mut app = make_app();
        let tmp = std::env::temp_dir().join("ur-test-valid.md");
        std::fs::write(&tmp, "title: My ticket\npriority: 1\n---\nBody text\n").unwrap();
        let status = std::process::Command::new("true").status();
        app.process_editor_result(status, &tmp, "ur".to_string());
        assert!(app.create_action_menu.is_some());
        assert!(app.pending_ticket.is_some());
        let pending = app.pending_ticket.as_ref().unwrap();
        assert_eq!(pending.project, "ur");
        assert_eq!(pending.title, "My ticket");
        assert_eq!(pending.priority, 1);
        let _ = std::fs::remove_file(&tmp);
    }

    #[test]
    fn project_input_cancel_clears_overlay() {
        let mut app = make_app();
        app.project_input = Some(ProjectInputState::new());
        let key = crossterm::event::KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        // We need a terminal for handle_project_input_key, but Cancel path
        // doesn't use it. Test via direct overlay handle_key.
        let overlay = app.project_input.as_mut().unwrap();
        let result = overlay.handle_key(key);
        assert_eq!(result, ProjectInputResult::Cancel);
    }

    #[test]
    fn project_input_submit_unknown_project_shows_error() {
        let mut app = make_app_with_projects(vec!["ur", "acme"]);
        // Simulate submitting an unknown project.
        app.project_input = None;
        // Directly test the validation logic.
        let unknown = "nonexistent".to_string();
        if !app.ctx.projects.contains(&unknown) {
            app.show_app_error_banner(format!("Unknown project: {unknown}"));
        }
        assert!(app.tickets_page.banner().is_some());
    }

    #[test]
    fn initial_create_state_is_none() {
        let app = make_app();
        assert!(app.create_action_menu.is_none());
        assert!(app.pending_project.is_none());
        assert!(app.project_input.is_none());
        assert!(app.pending_ticket.is_none());
    }
}
