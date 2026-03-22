use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::context::TuiContext;
use crate::data::DataManager;
use crate::event::{AppEvent, EventReceiver};
use crate::keymap::Action;
use crate::page::{Page, PageResult, TabId};
use crate::pages::{FlowsPage, TicketsPage};
use crate::widgets::header::TabInfo;
use crate::widgets::{render_footer, render_header};

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
    key_repeat_interval: Duration,
    last_nav_time: Instant,
}

impl App {
    /// Create a new `App` with the given context and data manager.
    ///
    /// The initial active tab is `Tickets`.
    pub fn new(ctx: TuiContext, data_manager: DataManager, key_repeat_interval_ms: u64) -> Self {
        Self {
            active_tab: TabId::Tickets,
            tickets_page: TicketsPage::new(),
            flows_page: FlowsPage::new(),
            ctx,
            data_manager,
            should_quit: false,
            key_repeat_interval: Duration::from_millis(key_repeat_interval_ms),
            last_nav_time: Instant::now() - Duration::from_secs(1),
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
            let event = receiver.recv().await;
            match event {
                Some(AppEvent::Key(key)) => self.handle_key(key),
                Some(AppEvent::Tick) => self.handle_tick(),
                Some(AppEvent::DataReady(payload)) => self.handle_data_ready(payload),
                Some(AppEvent::Resize(_, _)) => {} // Just redraw below
                None => break,
            }

            if self.should_quit {
                break;
            }

            self.draw(terminal)?;
        }

        Ok(())
    }

    /// Handle a key event: check for Ctrl+C, resolve via keymap, dispatch.
    fn handle_key(&mut self, key: crossterm::event::KeyEvent) {
        // Ctrl+C always exits cleanly.
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
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
            ref a if is_navigation_action(a) => {
                if self.last_nav_time.elapsed() >= self.key_repeat_interval {
                    self.last_nav_time = Instant::now();
                    self.dispatch_to_page(action);
                }
            }
            other => self.dispatch_to_page(other),
        }
    }

    /// Handle a tick: refresh active page data if stale.
    fn handle_tick(&mut self) {
        if self.active_page().needs_data() {
            self.fetch_active_tab_data();
        }
    }

    /// Handle a DataReady event: route the payload to the relevant page.
    fn handle_data_ready(&mut self, payload: crate::data::DataPayload) {
        self.tickets_page.on_data(&payload);
        self.flows_page.on_data(&payload);
    }

    /// Dispatch an action to the active page and handle quit if returned.
    fn dispatch_to_page(&mut self, action: Action) {
        let result = self.active_page_mut().handle_action(action);
        if result == PageResult::Quit {
            self.should_quit = true;
        }
    }

    /// Switch the active tab and fetch data if the new page needs it.
    fn switch_tab(&mut self, tab: TabId) {
        if self.active_tab == tab {
            return;
        }
        self.active_tab = tab;
        if self.active_page().needs_data() {
            self.fetch_active_tab_data();
        }
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
        let chunks = Layout::vertical([
            Constraint::Length(1), // header
            Constraint::Fill(1),   // content
            Constraint::Length(1), // footer
        ])
        .split(area);

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

        render_header(chunks[0], buf, &self.ctx, &tabs, self.active_tab);
        self.active_page().render(chunks[1], buf, &self.ctx);
        render_footer(
            chunks[2],
            buf,
            &self.ctx,
            &self.active_page().footer_commands(),
        );
    }
}

/// Returns `true` for actions that should be throttled when a key is held.
fn is_navigation_action(action: &Action) -> bool {
    matches!(
        action,
        Action::NavigateUp | Action::NavigateDown | Action::PageLeft | Action::PageRight
    )
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
        TuiContext { theme, keymap }
    }

    fn make_app() -> App {
        let ctx = make_ctx();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let data_manager = DataManager::new(42069, tx);
        App::new(ctx, data_manager, ur_config::DEFAULT_KEY_REPEAT_INTERVAL_MS)
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
