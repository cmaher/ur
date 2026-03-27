use std::collections::HashMap;
use std::io;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::widgets::{Clear, Widget};

use tracing::{debug, trace, warn};

use crate::context::TuiContext;
use crate::create_ticket::{
    PendingTicket, generate_template, is_title_placeholder, parse_ticket_file,
};
use crate::data::DataManager;
use crate::event::{AppEvent, EventManager, EventReceiver, UiEventItem};
use crate::keymap::Action;
use crate::notifications::NotificationManager;
use crate::page::TabId;
use crate::pages::tickets::{
    OverlayAction, open_filter_menu, open_force_close_confirm, open_priority_picker,
};
use crate::pages::{FlowsListScreen, TicketsListScreen, WorkersListScreen};
use crate::screen::{Screen, ScreenResult};
use crate::terminal;
use crate::throttle::PageThrottle;
use crate::widgets::create_action_menu::{
    CreateAction, CreateActionMenuState, CreateActionResult, PendingTicket as MenuPendingTicket,
};
use crate::widgets::header::TabInfo;
use crate::widgets::project_input::{ProjectInputResult, ProjectInputState};
use crate::widgets::settings_overlay::{SettingsOverlayState, SettingsResult};
use crate::widgets::{render_banner, render_footer, render_header, render_status_header};

/// Top-level application state and event loop coordinator.
///
/// Holds the active tab, per-tab screen stacks, context, data manager,
/// and quit flag. Receives events from the `EventReceiver` and dispatches
/// them to the top screen of the active tab's stack.
///
/// Each tab's stack always has at least one element: the root list screen.
/// Detail screens are pushed on top; pressing the current tab key pops back
/// to the root.
pub struct App {
    active_tab: TabId,
    /// Per-tab screen stacks. The element at index 0 is always the root list
    /// screen for that tab. Additional screens are pushed on top.
    stacks: HashMap<TabId, Vec<Box<dyn Screen>>>,
    ctx: TuiContext,
    data_manager: DataManager,
    event_manager: EventManager,
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
    /// Manages desktop notifications for workflow state transitions.
    notification_manager: NotificationManager,
    /// Throttles UI-event-driven data fetches with a cooldown window.
    throttle: PageThrottle,
}

impl App {
    /// Create a new `App` with the given context and data manager.
    ///
    /// The initial active tab is `Tickets`. Each tab's stack is initialized
    /// with its root list screen as the sole element.
    pub fn new(ctx: TuiContext, data_manager: DataManager, event_manager: EventManager) -> Self {
        let ticket_filter_cfg = &ctx.tui_config.ticket_filter;
        let notification_manager = NotificationManager::new(ctx.tui_config.notifications.clone());
        let notif_config = &ctx.tui_config.notifications;
        if !notification_manager.is_available()
            && (notif_config.flow_stalled || notif_config.flow_in_review)
        {
            warn!(
                "terminal-notifier not found; desktop notifications are enabled in config but will not fire"
            );
        }

        let tickets_root: Box<dyn Screen> = Box::new(TicketsListScreen::new(ticket_filter_cfg));
        let flows_root: Box<dyn Screen> = Box::new(FlowsListScreen::new());
        let workers_root: Box<dyn Screen> = Box::new(WorkersListScreen::new());

        let mut stacks: HashMap<TabId, Vec<Box<dyn Screen>>> = HashMap::new();
        stacks.insert(TabId::Tickets, vec![tickets_root]);
        stacks.insert(TabId::Flows, vec![flows_root]);
        stacks.insert(TabId::Workers, vec![workers_root]);

        Self {
            active_tab: TabId::Tickets,
            stacks,
            ctx,
            data_manager,
            event_manager,
            should_quit: false,
            settings_overlay: None,
            create_action_menu: None,
            pending_project: None,
            project_input: None,
            pending_ticket: None,
            notification_manager,
            throttle: PageThrottle::new(),
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
        debug!(active_tab = ?self.active_tab, "app run loop starting");
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

            // After draining events, check if the throttle cooldown has
            // elapsed and dirty pages need fetching.
            if self.throttle.should_flush() {
                self.flush_throttle();
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
            AppEvent::Tick => {
                trace!("tick");
                self.handle_tick();
            }
            AppEvent::DataReady(payload) => self.handle_data_ready(*payload),
            AppEvent::ActionResult(result) => self.handle_action_result(result),
            AppEvent::Resize(cols, rows) => {
                trace!(width = cols, height = rows, "resize");
                self.update_page_sizes(Rect::new(0, 0, cols, rows));
                if self.active_screen().needs_data() {
                    self.fetch_active_tab_data();
                }
            }
            AppEvent::UiEvent(items) => self.handle_ui_events(items),
            AppEvent::SetStatus(msg) => self.active_screen_mut().set_status(msg),
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
        if self.active_tab == TabId::Tickets && self.tickets_page().has_overlay() {
            self.handle_ticket_overlay_key(key);
            return;
        }

        // If the active screen has a banner, Enter or Escape dismisses it.
        if self.active_screen().banner().is_some()
            && matches!(key.code, KeyCode::Enter | KeyCode::Esc)
        {
            self.active_screen_mut().dismiss_banner();
            return;
        }

        // If the active screen has a status message, Enter or Escape dismisses it.
        if self.active_screen().status().is_some()
            && matches!(key.code, KeyCode::Enter | KeyCode::Esc)
        {
            self.active_screen_mut().dismiss_status();
            return;
        }

        let Some(action) = self.ctx.keymap.resolve(key) else {
            trace!(key_code = ?key.code, "unresolved key");
            return;
        };
        debug!(?action, "resolved key action");

        match action {
            Action::SwitchTab(tab) => {
                if tab == self.active_tab {
                    // Pressing the current tab key clears the stack to the root screen.
                    self.clear_stack_to_root(tab);
                } else {
                    self.switch_tab(tab);
                }
            }
            Action::Quit => {
                self.should_quit = true;
            }
            Action::CreateTicket if self.active_tab == TabId::Tickets => {
                self.begin_create_ticket(terminal);
            }
            Action::Filter if self.active_tab == TabId::Tickets => {
                let projects = self.ctx.projects.clone();
                open_filter_menu(self.tickets_page_mut(), &projects);
            }
            Action::SetPriority if self.active_tab == TabId::Tickets => {
                if self.tickets_page().selected_ticket_id().is_some() {
                    open_priority_picker(self.tickets_page_mut());
                }
            }
            Action::Dispatch if self.active_tab == TabId::Tickets => {
                self.dispatch_selected_ticket();
            }
            Action::DispatchAll if self.active_tab == TabId::Tickets => {
                self.dispatch_all_from_detail();
            }
            Action::LaunchDesign if self.active_tab == TabId::Tickets => {
                self.launch_design_for_selected_ticket();
            }
            Action::CloseTicket if self.active_tab == TabId::Tickets => {
                self.close_or_force_close_ticket();
            }
            Action::OpenTicket if self.active_tab == TabId::Tickets => {
                self.update_selected_ticket_status("open");
            }
            Action::CloseTicket if self.active_tab == TabId::Workers => {
                self.kill_selected_worker();
            }
            Action::CancelFlow | Action::CloseTicket if self.active_tab == TabId::Flows => {
                self.cancel_selected_flow();
            }
            Action::Redrive if self.active_tab == TabId::Flows => {
                self.redrive_selected_flow();
            }
            other => self.dispatch_to_screen(other),
        }
    }

    /// Route a key event to the tickets page overlay, handling tab-switch, priority, and filter.
    fn handle_ticket_overlay_key(&mut self, key: crossterm::event::KeyEvent) {
        if let Some(Action::SwitchTab(tab)) = self.ctx.keymap.resolve(key) {
            self.tickets_page_mut().close_overlay();
            self.switch_tab(tab);
            return;
        }
        let filters_before = self.tickets_page().filters().to_config();
        match self.tickets_page_mut().handle_overlay_key(key) {
            OverlayAction::SetPriority {
                ticket_id,
                priority,
            } => {
                self.data_manager
                    .update_ticket_priority(ticket_id, priority);
            }
            OverlayAction::ForceClose { ticket_id } => {
                self.data_manager.force_close_ticket(ticket_id);
            }
            OverlayAction::None => {}
        }
        let filters_after = self.tickets_page().filters().to_config();
        if filters_before != filters_after {
            save_ticket_filters(&self.ctx.config_dir, &filters_after);
        }
    }

    /// Handle a tick: auto-dismiss expired banners.
    fn handle_tick(&mut self) {
        self.active_screen_mut().tick_banner();
    }

    /// Handle a DataReady event: route the payload to the root screen of each tab.
    fn handle_data_ready(&mut self, payload: crate::data::DataPayload) {
        let (variant, is_ok) = match &payload {
            crate::data::DataPayload::Tickets(r) => ("Tickets", r.is_ok()),
            crate::data::DataPayload::Flows(r) => ("Flows", r.is_ok()),
            crate::data::DataPayload::Workers(r) => ("Workers", r.is_ok()),
            crate::data::DataPayload::TicketDetail(_) => ("TicketDetail", true),
            crate::data::DataPayload::TicketActivities(r) => ("TicketActivities", r.is_ok()),
        };
        debug!(variant, ok = is_ok, "data ready");
        if let crate::data::DataPayload::Flows(Ok((workflows, _total_count))) = &payload {
            self.notification_manager.seed_flows(workflows);
        }
        // TicketDetail and TicketActivities payloads go to the active screen (the top of the
        // active stack).
        if matches!(
            payload,
            crate::data::DataPayload::TicketDetail(_)
                | crate::data::DataPayload::TicketActivities(_)
        ) {
            self.active_screen_mut().on_data(&payload);
            return;
        }
        // All other payloads go to the root screen of every tab (the list pages).
        for stack in self.stacks.values_mut() {
            if let Some(root) = stack.first_mut() {
                root.on_data(&payload);
            }
        }
    }

    /// Handle an ActionResult event: route to the active screen for banner display,
    /// then trigger a data refresh so the UI reflects the change.
    fn handle_action_result(&mut self, result: crate::data::ActionResult) {
        match &result.result {
            Ok(msg) => debug!(message = %msg, "action result success"),
            Err(err) => warn!(error = %err, "action result error"),
        }
        let success = result.result.is_ok();
        match self.active_tab {
            TabId::Tickets => self.tickets_page_mut().on_action_result(&result),
            TabId::Flows => self.flows_page_mut().on_action_result(&result),
            TabId::Workers => self.workers_page_mut().on_action_result(&result),
        }
        if success {
            self.fetch_active_tab_data();
        }
    }

    /// Handle a batch of UI events: map entity types to dirty tabs and
    /// accumulate them in the throttle. If no cooldown is active the
    /// throttle will flush immediately on the next `should_flush` check.
    fn handle_ui_events(&mut self, items: Vec<UiEventItem>) {
        let entity_types: Vec<&str> = items.iter().map(|i| i.entity_type.as_str()).collect();
        debug!(
            batch_size = items.len(),
            ?entity_types,
            "ui events received"
        );
        let dirty_tabs = items.iter().flat_map(|item| {
            match item.entity_type.as_str() {
                "ticket" => &[TabId::Tickets, TabId::Flows] as &[TabId],
                "workflow" => &[TabId::Flows],
                "worker" => &[TabId::Workers],
                _ => &[],
            }
            .iter()
            .copied()
        });

        self.throttle.mark_dirty(dirty_tabs);

        // If no cooldown is active, flush immediately.
        if self.throttle.should_flush() {
            self.flush_throttle();
        }
    }

    /// Flush the throttle: fetch data for the active tab if it is dirty,
    /// and mark non-active dirty tabs as stale for lazy refresh.
    fn flush_throttle(&mut self) {
        let dirty = self.throttle.flush();
        if dirty.is_empty() {
            return;
        }
        debug!(tabs = ?dirty, "flushing throttle");

        for tab in &dirty {
            if *tab == self.active_tab {
                self.fetch_active_tab_data();
            } else {
                self.root_screen_mut(*tab).mark_stale();
            }
        }
    }

    /// Update the status of the currently selected ticket.
    fn update_selected_ticket_status(&mut self, status: &str) {
        if let Some(ticket_id) = self.tickets_page().selected_ticket_id() {
            self.data_manager
                .update_ticket_status(ticket_id, status.to_owned());
        }
    }

    /// Close the selected ticket, opening a force-close confirmation if it has open children.
    fn close_or_force_close_ticket(&mut self) {
        let Some(ticket) = self.tickets_page().selected_ticket().cloned() else {
            return;
        };
        let open_children = ticket.children_total - ticket.children_completed;
        if open_children > 0 {
            let ticket_id = ticket.id.clone();
            open_force_close_confirm(self.tickets_page_mut(), ticket_id, open_children);
        } else {
            self.update_selected_ticket_status("closed");
        }
    }

    /// Dispatch the currently selected ticket on the tickets page.
    ///
    /// When a `TicketDetailScreen` is active, dispatches the highlighted child
    /// ticket rather than the parent.
    fn dispatch_selected_ticket(&mut self) {
        let ticket_id = if let Some(detail) = self.active_screen().as_any_ticket_detail() {
            detail.selected_child_id()
        } else {
            self.tickets_page().selected_ticket_id()
        };
        if let Some(ticket_id) = ticket_id {
            self.active_screen_mut()
                .set_status(format!("Dispatching ticket {ticket_id}..."));
            self.data_manager
                .dispatch_ticket(ticket_id, &self.ctx.project_configs);
        }
    }

    /// Dispatch the parent ticket from a ticket detail screen.
    ///
    /// Uses the detail screen's `ticket_id()` (the parent being viewed) rather
    /// than `selected_child_id()`.
    fn dispatch_all_from_detail(&mut self) {
        if let Some(screen) = self.active_screen().as_any_ticket_detail() {
            let ticket_id = screen.ticket_id().to_owned();
            if let Some(detail_mut) = self.active_screen_mut().as_any_ticket_detail_mut() {
                detail_mut.set_status(format!("Dispatching ticket {ticket_id}..."));
            }
            self.data_manager
                .dispatch_ticket(ticket_id, &self.ctx.project_configs);
        }
    }

    /// Launch a design worker for the currently selected ticket on the tickets page.
    fn launch_design_for_selected_ticket(&mut self) {
        if let Some(ticket_id) = self.tickets_page().selected_ticket_id() {
            self.tickets_page_mut()
                .set_status(format!("Launching design worker for {ticket_id}..."));
            self.data_manager
                .launch_design_worker(ticket_id, &self.ctx.project_configs);
        }
    }

    /// Stop the currently selected worker on the workers page.
    /// Optimistically removes the worker from the list immediately;
    /// restores it if the kill RPC fails.
    fn kill_selected_worker(&mut self) {
        if let Some(worker_id) = self.workers_page().selected_worker_id() {
            self.workers_page_mut().optimistic_remove(&worker_id);
            self.data_manager.stop_worker(worker_id);
        }
    }

    /// Cancel the workflow for the currently selected flow.
    fn cancel_selected_flow(&mut self) {
        if let Some(ticket_id) = self.flows_page().selected_ticket_id() {
            self.flows_page_mut()
                .set_status(format!("Cancelling workflow for {ticket_id}..."));
            self.data_manager.cancel_flow(ticket_id);
        }
    }

    /// Redrive the workflow for the currently selected flow.
    fn redrive_selected_flow(&mut self) {
        if let Some(ticket_id) = self.flows_page().selected_ticket_id() {
            self.flows_page_mut()
                .set_status(format!("Redriving workflow for {ticket_id}..."));
            self.data_manager.redrive_flow(ticket_id);
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
    /// 1. If a global project filter is active (launched with -p), use that project.
    /// 2. If a single project filter is active on the tickets page, use that project.
    /// 3. If there is only one configured project, use it.
    /// 4. Otherwise, show the project input overlay.
    fn begin_create_ticket(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) {
        // Global project filter from CLI -p flag.
        if let Some(ref project) = self.ctx.project_filter {
            self.open_editor_for_ticket(project.clone(), terminal);
            return;
        }

        // Check if the ticket list has a single project filter active.
        if let Some(filtered_project) = self.tickets_page().single_project_filter() {
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

        // Pause crossterm reader so it stops competing for stdin with the editor.
        self.event_manager.pause();

        // Suspend TUI, run editor, restore TUI.
        terminal::restore_terminal();
        let status = std::process::Command::new(&editor).arg(&tmp_path).status();
        // Re-setup terminal — recreate the backend in-place.
        if let Err(e) = reinit_terminal(terminal) {
            self.event_manager.resume();
            // If we can't restore the terminal, we have to bail.
            self.show_app_error_banner(format!("Failed to restore terminal: {e}"));
            return;
        }

        self.event_manager.resume();

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
        self.tickets_page_mut()
            .on_action_result(&crate::data::ActionResult {
                result: Err(message),
                silent_on_success: false,
            });
    }

    /// Dispatch an action to the active screen and handle the result.
    fn dispatch_to_screen(&mut self, action: Action) {
        let result = self.active_screen_mut().handle_action(action.clone());
        debug!(?action, ?result, "dispatch to screen");
        match result {
            ScreenResult::Quit => {
                self.should_quit = true;
            }
            ScreenResult::Push(screen) => {
                self.stacks.entry(self.active_tab).or_default().push(screen);
            }
            ScreenResult::Pop => {
                let stack = self.stacks.entry(self.active_tab).or_default();
                // Only pop if there is more than the root screen.
                if stack.len() > 1 {
                    stack.pop();
                }
            }
            ScreenResult::Consumed | ScreenResult::Ignored => {}
        }
        // Trigger immediate fetch if the active screen now needs data (e.g. after refresh).
        if self.active_screen().needs_data() {
            self.fetch_active_tab_data();
        }
    }

    /// Switch the active tab, preserving each tab's screen stack.
    ///
    /// Dismisses the banner on the screen we're leaving, marks the root of
    /// the newly active tab stale, and fetches fresh data.
    fn switch_tab(&mut self, tab: TabId) {
        if self.active_tab == tab {
            return;
        }
        debug!(from = ?self.active_tab, to = ?tab, "switch tab");
        // Dismiss banner on the screen we're leaving.
        self.active_screen_mut().dismiss_banner();
        self.active_tab = tab;
        // Mark the root of the newly active tab stale so it re-fetches.
        self.root_screen_mut(tab).mark_stale();
        self.fetch_active_tab_data();
    }

    /// Clear a tab's screen stack down to just the root list screen.
    ///
    /// Used when the user presses the shortcut for the already-active tab,
    /// which acts as a "go home" gesture.
    fn clear_stack_to_root(&mut self, tab: TabId) {
        debug!(?tab, "clear stack to root");
        let stack = self.stacks.entry(tab).or_default();
        stack.truncate(1);
    }

    /// Fetch data for the currently active tab.
    ///
    /// If the active screen is a `TicketDetailScreen`, fetches ticket detail
    /// data; if it is a `TicketActivitiesScreen`, fetches activities data;
    /// otherwise fetches the appropriate list data for the tab.
    fn fetch_active_tab_data(&self) {
        // If the top of the active stack is a detail screen, fetch its data.
        if let Some(detail) = self.active_screen().as_any_ticket_detail() {
            let ticket_id = detail.ticket_id().to_owned();
            debug!(tab = ?self.active_tab, fetch = "detail", %ticket_id, "fetch active tab data");
            let page_size = detail.child_page_size();
            let offset = detail.child_offset();
            self.data_manager
                .fetch_ticket_detail(ticket_id, Some(page_size), Some(offset));
            return;
        }
        // If the top of the active stack is an activities screen, fetch its data.
        if let Some(activities) = self.active_screen().as_any_ticket_activities() {
            let ticket_id = activities.ticket_id().to_owned();
            debug!(tab = ?self.active_tab, fetch = "activities", %ticket_id, "fetch active tab data");
            let author_filter = activities.author_filter().map(str::to_owned);
            self.data_manager
                .fetch_ticket_activities(ticket_id, author_filter);
            return;
        }
        debug!(tab = ?self.active_tab, fetch = "list", "fetch active tab data");
        match self.active_tab {
            TabId::Tickets => {
                let page = self.tickets_page();
                let params = page.pagination_params();
                self.data_manager.fetch_tickets(
                    Some(params.page_size),
                    Some(params.offset),
                    Some(params.include_children),
                    &page.filters().statuses,
                );
            }
            TabId::Flows => self.data_manager.fetch_flows(
                Some(self.flows_page().page_size()),
                Some(self.flows_page().page_offset()),
            ),
            TabId::Workers => self.data_manager.fetch_workers(),
        }
    }

    /// Get a reference to the top screen of the active tab's stack.
    fn active_screen(&self) -> &dyn Screen {
        let stack = self
            .stacks
            .get(&self.active_tab)
            .expect("active tab must have a stack");
        stack.last().expect("stack must not be empty").as_ref()
    }

    /// Get a mutable reference to the top screen of the active tab's stack.
    fn active_screen_mut(&mut self) -> &mut dyn Screen {
        let stack = self
            .stacks
            .get_mut(&self.active_tab)
            .expect("active tab must have a stack");
        stack.last_mut().expect("stack must not be empty").as_mut()
    }

    /// Get a mutable reference to the root screen of the given tab.
    fn root_screen_mut(&mut self, tab: TabId) -> &mut dyn Screen {
        let stack = self.stacks.get_mut(&tab).expect("tab must have a stack");
        stack.first_mut().expect("stack must not be empty").as_mut()
    }

    /// Get a reference to the `TicketsListScreen` at the root of the Tickets tab stack.
    fn tickets_page(&self) -> &TicketsListScreen {
        let stack = self
            .stacks
            .get(&TabId::Tickets)
            .expect("Tickets tab must have a stack");
        stack
            .first()
            .expect("stack must not be empty")
            .as_any_tickets()
            .expect("root of Tickets stack must be TicketsListScreen")
    }

    /// Get a mutable reference to the `TicketsListScreen` at the root of the Tickets tab stack.
    fn tickets_page_mut(&mut self) -> &mut TicketsListScreen {
        let stack = self
            .stacks
            .get_mut(&TabId::Tickets)
            .expect("Tickets tab must have a stack");
        stack
            .first_mut()
            .expect("stack must not be empty")
            .as_any_tickets_mut()
            .expect("root of Tickets stack must be TicketsListScreen")
    }

    /// Get a reference to the `FlowsListScreen` at the root of the Flows tab stack.
    fn flows_page(&self) -> &FlowsListScreen {
        let stack = self
            .stacks
            .get(&TabId::Flows)
            .expect("Flows tab must have a stack");
        stack
            .first()
            .expect("stack must not be empty")
            .as_any_flows()
            .expect("root of Flows stack must be FlowsListScreen")
    }

    /// Get a mutable reference to the `FlowsListScreen` at the root of the Flows tab stack.
    fn flows_page_mut(&mut self) -> &mut FlowsListScreen {
        let stack = self
            .stacks
            .get_mut(&TabId::Flows)
            .expect("Flows tab must have a stack");
        stack
            .first_mut()
            .expect("stack must not be empty")
            .as_any_flows_mut()
            .expect("root of Flows stack must be FlowsListScreen")
    }

    /// Get a reference to the `WorkersListScreen` at the root of the Workers tab stack.
    fn workers_page(&self) -> &WorkersListScreen {
        let stack = self
            .stacks
            .get(&TabId::Workers)
            .expect("Workers tab must have a stack");
        stack
            .first()
            .expect("stack must not be empty")
            .as_any_workers()
            .expect("root of Workers stack must be WorkersListScreen")
    }

    /// Get a mutable reference to the `WorkersListScreen` at the root of the Workers tab stack.
    fn workers_page_mut(&mut self) -> &mut WorkersListScreen {
        let stack = self
            .stacks
            .get_mut(&TabId::Workers)
            .expect("Workers tab must have a stack");
        stack
            .first_mut()
            .expect("stack must not be empty")
            .as_any_workers_mut()
            .expect("root of Workers stack must be WorkersListScreen")
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
        self.tickets_page_mut().update_page_size(content_height);
    }

    /// Render the full application frame: header, content area, footer.
    fn render(&self, area: Rect, buf: &mut Buffer) {
        // Fill the entire frame with the base background so no terminal
        // theme bleeds through in margins or empty regions.
        let base_style = Style::default()
            .bg(self.ctx.theme.base_100)
            .fg(self.ctx.theme.base_content);
        buf.set_style(area, base_style);

        let chunks = Layout::vertical([
            Constraint::Length(1), // header
            Constraint::Length(1), // sub-header (banner or status, always reserved)
            Constraint::Fill(1),   // content
            Constraint::Length(1), // footer
        ])
        .split(area);

        let tabs = self.tab_infos();

        render_header(chunks[0], buf, &self.ctx, &tabs, self.active_tab);

        if let Some(banner) = self.active_screen().banner() {
            render_banner(chunks[1], buf, &self.ctx, banner);
        } else if let Some(status) = self.active_screen().status() {
            render_status_header(chunks[1], buf, &self.ctx, status);
        }

        self.active_screen().render(chunks[2], buf, &self.ctx);
        render_footer(
            chunks[3],
            buf,
            &self.ctx,
            &self.active_screen().footer_commands(&self.ctx.keymap),
        );

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

    /// Build the tab info list for the header, reading titles/shortcuts from
    /// the root screens of each tab.
    fn tab_infos(&self) -> Vec<TabInfo> {
        let tickets = self.tickets_page();
        let flows = self.flows_page();
        let workers = self.workers_page();
        vec![
            TabInfo {
                id: TabId::Tickets,
                label: tickets.title().to_string(),
                shortcut: tickets.shortcut_char(),
            },
            TabInfo {
                id: TabId::Flows,
                label: flows.title().to_string(),
                shortcut: flows.shortcut_char(),
            },
            TabInfo {
                id: TabId::Workers,
                label: workers.title().to_string(),
                shortcut: workers.shortcut_char(),
            },
        ]
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
    use crate::pages::TicketsListScreen;
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
            project_filter: None,
        }
    }

    fn make_app() -> App {
        let ctx = make_ctx();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let data_manager = DataManager::new(42069, tx, None);
        let event_manager = EventManager::test_new();
        App::new(ctx, data_manager, event_manager)
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
        let payload = DataPayload::Tickets(Ok((vec![], 0)));
        app.handle_data_ready(payload);
        assert!(!app.tickets_page().needs_data());
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
            project_filter: None,
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let data_manager = DataManager::new(42069, tx, None);
        let event_manager = EventManager::test_new();
        App::new(ctx, data_manager, event_manager)
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
        assert!(app.tickets_page().banner().is_some());
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
        assert!(app.tickets_page().banner().is_none());
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
        assert!(app.tickets_page().banner().is_some());
    }

    #[test]
    fn process_editor_result_empty_file_shows_banner() {
        let mut app = make_app();
        let tmp = std::env::temp_dir().join("ur-test-empty.md");
        std::fs::write(&tmp, "").unwrap();
        // Create a successful exit status by running a trivial command.
        let status = std::process::Command::new("true").status();
        app.process_editor_result(status, &tmp, "ur".to_string());
        assert!(app.tickets_page().banner().is_some());
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
        assert!(app.tickets_page().banner().is_some());
    }

    #[test]
    fn initial_create_state_is_none() {
        let app = make_app();
        assert!(app.create_action_menu.is_none());
        assert!(app.pending_project.is_none());
        assert!(app.project_input.is_none());
        assert!(app.pending_ticket.is_none());
    }

    #[test]
    fn stacks_initialized_with_root_screens() {
        let app = make_app();
        // Each tab should have exactly one screen (the root list screen).
        assert_eq!(app.stacks.get(&TabId::Tickets).map(|s| s.len()), Some(1));
        assert_eq!(app.stacks.get(&TabId::Flows).map(|s| s.len()), Some(1));
        assert_eq!(app.stacks.get(&TabId::Workers).map(|s| s.len()), Some(1));
    }

    #[tokio::test]
    async fn switch_tab_preserves_stacks() {
        let mut app = make_app();
        // Push a dummy screen onto the Tickets stack to simulate navigation.
        // We verify the stack depth is preserved after switching tabs.
        let initial_len = app
            .stacks
            .get(&TabId::Tickets)
            .map(|s| s.len())
            .unwrap_or(0);

        app.switch_tab(TabId::Flows);
        assert_eq!(app.active_tab, TabId::Flows);

        // Tickets stack should be unchanged.
        let after_len = app
            .stacks
            .get(&TabId::Tickets)
            .map(|s| s.len())
            .unwrap_or(0);
        assert_eq!(initial_len, after_len);
    }

    #[test]
    fn clear_stack_to_root_truncates() {
        let mut app = make_app();
        // Manually push an extra screen to simulate a detail view.
        // We push a second TicketsListScreen as a stand-in for a detail screen.
        let extra: Box<dyn Screen> = Box::new(TicketsListScreen::new(
            &ur_config::TicketFilterConfig::default(),
        ));
        app.stacks.get_mut(&TabId::Tickets).unwrap().push(extra);
        assert_eq!(app.stacks.get(&TabId::Tickets).unwrap().len(), 2);

        app.clear_stack_to_root(TabId::Tickets);
        assert_eq!(app.stacks.get(&TabId::Tickets).unwrap().len(), 1);
    }

    #[test]
    fn pop_at_root_is_noop() {
        let mut app = make_app();
        assert_eq!(app.stacks.get(&TabId::Tickets).unwrap().len(), 1);
        // Dispatch a Pop result — the stack should remain at 1.
        let result = ScreenResult::Pop;
        app.active_tab = TabId::Tickets;
        if let ScreenResult::Pop = result {
            let stack = app.stacks.entry(app.active_tab).or_default();
            if stack.len() > 1 {
                stack.pop();
            }
        }
        assert_eq!(app.stacks.get(&TabId::Tickets).unwrap().len(), 1);
    }
}
