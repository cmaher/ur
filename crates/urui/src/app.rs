use std::io;

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;

use crate::context::TuiContext;
use crate::data::DataManager;
use crate::event::{AppEvent, EventReceiver};
use crate::keymap::Action;
use crate::page::{Page, PageResult, TabId};
use crate::pages::tickets::{open_filter_menu, open_priority_picker};
use crate::pages::{FlowsPage, TicketsPage};
use crate::widgets::header::TabInfo;
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
    ctx: TuiContext,
    data_manager: DataManager,
    should_quit: bool,
    /// Global settings overlay state, present when the overlay is open.
    settings_overlay: Option<SettingsOverlayState>,
}

impl App {
    /// Create a new `App` with the given context and data manager.
    ///
    /// The initial active tab is `Tickets`.
    pub fn new(ctx: TuiContext, data_manager: DataManager) -> Self {
        Self {
            active_tab: TabId::Tickets,
            tickets_page: TicketsPage::new(),
            flows_page: FlowsPage::new(),
            ctx,
            data_manager,
            should_quit: false,
            settings_overlay: None,
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
            self.process_event(first);

            // Drain any queued events without blocking so we batch
            // multiple key presses into a single redraw.
            while let Ok(ev) = receiver.try_recv() {
                self.process_event(ev);
            }

            if self.should_quit {
                break;
            }

            self.draw(terminal)?;
        }

        Ok(())
    }

    /// Process a single event (key, tick, data, resize, action result).
    fn process_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Tick => self.handle_tick(),
            AppEvent::DataReady(payload) => self.handle_data_ready(payload),
            AppEvent::ActionResult(result) => self.handle_action_result(result),
            AppEvent::Resize(_, _) => {} // Just redraw
        }
    }

    /// Handle a key event: check for Ctrl+C, dismiss banners, resolve via keymap, dispatch.
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        // Ctrl+C always exits cleanly.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
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
            if let Some((ticket_id, priority)) = self.tickets_page.handle_overlay_key(key) {
                self.data_manager
                    .update_ticket_priority(ticket_id, priority);
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
        }
    }

    /// Get a reference to the currently active page.
    fn active_page(&self) -> &dyn Page {
        match self.active_tab {
            TabId::Tickets => &self.tickets_page,
            TabId::Flows => &self.flows_page,
        }
    }

    /// Get a mutable reference to the currently active page.
    fn active_page_mut(&mut self) -> &mut dyn Page {
        match self.active_tab {
            TabId::Tickets => &mut self.tickets_page,
            TabId::Flows => &mut self.flows_page,
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

        let has_status = self.active_page().status().is_some();
        let chunks = if has_status {
            Layout::vertical([
                Constraint::Length(1), // header
                Constraint::Length(1), // status header
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
        ];

        if let Some(banner) = self.active_page().banner() {
            render_banner(chunks[0], buf, &self.ctx, banner);
        } else {
            render_header(chunks[0], buf, &self.ctx, &tabs, self.active_tab);
        }

        if has_status {
            if let Some(status) = self.active_page().status() {
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

        // Render settings overlay on top if open.
        if let Some(ref overlay) = self.settings_overlay {
            overlay.render(area, buf, &self.ctx);
            // Override footer with settings overlay commands.
            let footer_area = *chunks.last().expect("chunks must have footer");
            let bg_style = Style::default()
                .bg(self.ctx.theme.neutral)
                .fg(self.ctx.theme.neutral_content);
            buf.set_style(footer_area, bg_style);
            render_footer(footer_area, buf, &self.ctx, &overlay.footer_commands());
        }
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
        let key = crossterm::event::KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::SHIFT);
        app.handle_key(key);
        assert!(app.should_quit);
    }

    #[test]
    fn handle_ctrl_c() {
        let mut app = make_app();
        let key = crossterm::event::KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        app.handle_key(key);
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
        let key = crossterm::event::KeyEvent::new(KeyCode::Char('f'), KeyModifiers::NONE);
        app.handle_key(key);
        assert_eq!(app.active_tab, TabId::Flows);
    }
}
