use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::context::TuiContext;
use crate::data::DataPayload;
use crate::keymap::{Action, Keymap};
use crate::page::{Banner, FooterCommand, Page, PageResult, StatusMessage};
use crate::pages::{FlowsListScreen, TicketDetailScreen, TicketsListScreen, WorkersListScreen};

/// Result of a screen handling an action.
pub enum ScreenResult {
    /// The screen consumed the action; no further handling needed.
    Consumed,
    /// The screen did not handle the action; propagate to the app.
    Ignored,
    /// The user requested to quit.
    Quit,
    /// Push a new screen onto the active tab's stack.
    Push(Box<dyn Screen>),
    /// Pop the current screen, returning to the previous one.
    Pop,
}

impl std::fmt::Debug for ScreenResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScreenResult::Consumed => write!(f, "Consumed"),
            ScreenResult::Ignored => write!(f, "Ignored"),
            ScreenResult::Quit => write!(f, "Quit"),
            ScreenResult::Push(_) => write!(f, "Push(...)"),
            ScreenResult::Pop => write!(f, "Pop"),
        }
    }
}

impl PartialEq for ScreenResult {
    fn eq(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (ScreenResult::Consumed, ScreenResult::Consumed)
                | (ScreenResult::Ignored, ScreenResult::Ignored)
                | (ScreenResult::Quit, ScreenResult::Quit)
                | (ScreenResult::Push(_), ScreenResult::Push(_))
                | (ScreenResult::Pop, ScreenResult::Pop)
        )
    }
}

/// Trait implemented by every screen in the TUI.
///
/// Screens are stacked per-tab. The bottom of each tab's stack is always the
/// root list screen. The app delegates all input and rendering to the top of
/// the active tab's stack.
pub trait Screen: Send {
    /// Handle a resolved action. Returns how the action was handled.
    fn handle_action(&mut self, action: Action) -> ScreenResult;

    /// Render the screen content into the given area.
    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext);

    /// Footer commands available while this screen is active.
    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand>;

    /// Receive fetched data from the data layer.
    fn on_data(&mut self, payload: &DataPayload);

    /// Whether this screen currently needs a data fetch.
    fn needs_data(&self) -> bool;

    /// Mark this screen's data as stale so the next tick triggers a re-fetch.
    fn mark_stale(&mut self);

    /// Returns the active banner for this screen, if any.
    fn banner(&self) -> Option<&Banner> {
        None
    }

    /// Dismiss any active banner on this screen.
    fn dismiss_banner(&mut self) {}

    /// Tick the banner timer, auto-dismissing expired banners.
    fn tick_banner(&mut self) {}

    /// Returns the active status message for this screen, if any.
    fn status(&self) -> Option<&StatusMessage> {
        None
    }

    /// Dismiss the active status message on this screen.
    fn dismiss_status(&mut self) {}

    /// Set the status message to the given text (for intermediate progress updates).
    fn set_status(&mut self, _text: String) {}

    /// Clear the status message (called when the async action completes).
    fn clear_status(&mut self) {}

    /// Downcast to `TicketsListScreen` if this is one.
    fn as_any_tickets(&self) -> Option<&TicketsListScreen> {
        None
    }

    /// Mutably downcast to `TicketsListScreen` if this is one.
    fn as_any_tickets_mut(&mut self) -> Option<&mut TicketsListScreen> {
        None
    }

    /// Downcast to `FlowsListScreen` if this screen is one.
    fn as_any_flows(&self) -> Option<&FlowsListScreen> {
        None
    }

    /// Mutably downcast to `FlowsListScreen` if this screen is one.
    fn as_any_flows_mut(&mut self) -> Option<&mut FlowsListScreen> {
        None
    }

    /// Downcast to `WorkersListScreen` if this screen is one.
    fn as_any_workers(&self) -> Option<&WorkersListScreen> {
        None
    }

    /// Mutably downcast to `WorkersListScreen` if this screen is one.
    fn as_any_workers_mut(&mut self) -> Option<&mut WorkersListScreen> {
        None
    }

    /// Downcast to `TicketDetailScreen` if this screen is one.
    fn as_any_ticket_detail(&self) -> Option<&TicketDetailScreen> {
        None
    }

    /// Mutably downcast to `TicketDetailScreen` if this screen is one.
    fn as_any_ticket_detail_mut(&mut self) -> Option<&mut TicketDetailScreen> {
        None
    }
}

/// A generic adapter wrapping any `Page + Send` implementor as a `Screen`.
///
/// Useful for pushing ad-hoc screens onto the stack in tests or future detail
/// screens. Does not provide typed downcast methods — use the concrete screen
/// types (`TicketsListScreen`, `FlowsListScreen`, `WorkersListScreen`) for the
/// root list screens.
pub struct PageScreenAdapter<P: Page + Send> {
    page: P,
}

impl<P: Page + Send> PageScreenAdapter<P> {
    pub fn new(page: P) -> Self {
        Self { page }
    }

    /// Access the inner page directly.
    pub fn inner(&self) -> &P {
        &self.page
    }

    /// Mutably access the inner page directly.
    pub fn inner_mut(&mut self) -> &mut P {
        &mut self.page
    }
}

impl<P: Page + Send> Screen for PageScreenAdapter<P> {
    fn handle_action(&mut self, action: Action) -> ScreenResult {
        match self.page.handle_action(action) {
            PageResult::Consumed => ScreenResult::Consumed,
            PageResult::Ignored => ScreenResult::Ignored,
            PageResult::Quit => ScreenResult::Quit,
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext) {
        self.page.render(area, buf, ctx);
    }

    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand> {
        self.page.footer_commands(keymap)
    }

    fn on_data(&mut self, payload: &DataPayload) {
        self.page.on_data(payload);
    }

    fn needs_data(&self) -> bool {
        self.page.needs_data()
    }

    fn mark_stale(&mut self) {
        self.page.mark_stale();
    }

    fn banner(&self) -> Option<&Banner> {
        self.page.banner()
    }

    fn dismiss_banner(&mut self) {
        self.page.dismiss_banner();
    }

    fn tick_banner(&mut self) {
        self.page.tick_banner();
    }

    fn status(&self) -> Option<&StatusMessage> {
        self.page.status()
    }

    fn dismiss_status(&mut self) {
        self.page.dismiss_status();
    }

    fn set_status(&mut self, text: String) {
        self.page.set_status(text);
    }

    fn clear_status(&mut self) {
        self.page.clear_status();
    }
}
