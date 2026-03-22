use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::context::TuiContext;
use crate::data::DataPayload;
use crate::keymap::Action;

/// Identifies which tab/page is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabId {
    Tickets,
    Flows,
}

/// Result of a page handling an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageResult {
    /// The page consumed the action; no further handling needed.
    Consumed,
    /// The page did not handle the action; propagate to the app.
    Ignored,
    /// The user requested to quit.
    Quit,
}

/// A command displayed in the footer bar for the active page.
pub struct FooterCommand {
    /// Short label shown next to the key (e.g. "q").
    pub key_label: String,
    /// Human-readable description (e.g. "Quit").
    pub description: String,
}

/// Trait implemented by every tab page in the TUI.
pub trait Page {
    /// The unique tab identifier for this page.
    fn tab_id(&self) -> TabId;

    /// Display title shown in the header tab bar.
    fn title(&self) -> &str;

    /// The character used as a keyboard shortcut to switch to this tab.
    fn shortcut_char(&self) -> char;

    /// Handle a resolved action. Returns how the action was handled.
    fn handle_action(&mut self, action: Action) -> PageResult;

    /// Render the page content into the given area.
    fn render(&self, area: Rect, buf: &mut Buffer, ctx: &TuiContext);

    /// Footer commands available while this page is active.
    fn footer_commands(&self) -> Vec<FooterCommand>;

    /// Receive fetched data from the data layer.
    fn on_data(&mut self, payload: &DataPayload);

    /// Whether this page currently needs a data fetch.
    fn needs_data(&self) -> bool;
}
