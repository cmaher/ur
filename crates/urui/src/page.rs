use std::time::Instant;

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

/// A command displayed in the footer bar for the active page.
pub struct FooterCommand {
    /// Short label shown next to the key (e.g. "q").
    pub key_label: String,
    /// Human-readable description (e.g. "Quit").
    pub description: String,
    /// Whether this is a common command (rendered on the right side).
    pub common: bool,
}
