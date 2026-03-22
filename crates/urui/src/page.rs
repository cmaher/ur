use std::time::Instant;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::context::TuiContext;
use crate::data::DataPayload;
use crate::keymap::{Action, Keymap};

/// Duration after which success banners auto-dismiss.
const BANNER_AUTO_DISMISS_SECS: u64 = 5;

/// Visual variant controlling banner color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BannerVariant {
    Success,
    Error,
}

/// An in-progress status message displayed below the tab header.
#[derive(Debug, Clone)]
pub struct StatusMessage {
    pub text: String,
    /// Whether the user can dismiss this status with Esc or Enter.
    pub dismissable: bool,
}

/// A temporary notification banner displayed in the header slot.
#[derive(Debug, Clone)]
pub struct Banner {
    pub message: String,
    pub variant: BannerVariant,
    pub created_at: Instant,
}

impl Banner {
    /// Returns true if this banner should be auto-dismissed based on elapsed time.
    pub fn is_expired(&self) -> bool {
        match self.variant {
            BannerVariant::Success => {
                self.created_at.elapsed().as_secs() >= BANNER_AUTO_DISMISS_SECS
            }
            BannerVariant::Error => false,
        }
    }
}

/// Identifies which tab/page is active.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TabId {
    Tickets,
    Flows,
    Workers,
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
    /// Whether this is a common command (rendered on the right side).
    pub common: bool,
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
    fn footer_commands(&self, keymap: &Keymap) -> Vec<FooterCommand>;

    /// Receive fetched data from the data layer.
    fn on_data(&mut self, payload: &DataPayload);

    /// Whether this page currently needs a data fetch.
    fn needs_data(&self) -> bool;

    /// Returns the active banner for this page, if any.
    fn banner(&self) -> Option<&Banner> {
        None
    }

    /// Dismiss any active banner on this page.
    fn dismiss_banner(&mut self) {}

    /// Tick the banner timer, auto-dismissing expired banners.
    fn tick_banner(&mut self) {}

    /// Returns the active status message for this page, if any.
    fn status(&self) -> Option<&StatusMessage> {
        None
    }

    /// Dismiss the active status message on this page.
    fn dismiss_status(&mut self) {}

    /// Clear the status message (called when the async action completes).
    fn clear_status(&mut self) {}

    /// Mark this page's data as stale so the next tick triggers a re-fetch.
    fn mark_stale(&mut self);
}
