use std::cell::Cell;
use std::collections::HashSet;
use std::time::{Duration, Instant};

use ur_rpc::proto::core::WorkerSummary;
use ur_rpc::proto::ticket::{ActivityEntry, GetTicketResponse, Ticket, WorkflowInfo};

use super::components::banner::BannerVariant;
use super::input::{GlobalHandler, InputStack};
use super::navigation::{NavigationModel, TabId};
use super::notifications::NotificationModel;

/// Duration after which success banners auto-dismiss.
const BANNER_AUTO_DISMISS_SECS: u64 = 5;

/// Cooldown duration between batched UI event fetches.
const THROTTLE_COOLDOWN: Duration = Duration::from_millis(200);

/// Tracks the loading state of asynchronous data.
///
/// Each page sub-model uses `LoadState<T>` to represent whether its data
/// has not been fetched yet, is currently loading, has been successfully
/// loaded, or failed to load.
#[derive(Debug, Clone)]
pub enum LoadState<T> {
    /// Data has not been requested yet.
    NotLoaded,
    /// A fetch is in progress.
    Loading,
    /// Data was successfully loaded.
    Loaded(T),
    /// The fetch failed with an error message.
    Error(String),
}

impl<T> LoadState<T> {
    /// Returns `true` if the state is `Loaded`.
    pub fn is_loaded(&self) -> bool {
        matches!(self, LoadState::Loaded(_))
    }

    /// Returns `true` if the state is `Loading`.
    pub fn is_loading(&self) -> bool {
        matches!(self, LoadState::Loading)
    }

    /// Returns a reference to the loaded data, if available.
    pub fn data(&self) -> Option<&T> {
        match self {
            LoadState::Loaded(data) => Some(data),
            _ => None,
        }
    }
}

/// Data loaded for the ticket list page.
#[derive(Debug, Clone)]
pub struct TicketListData {
    pub tickets: Vec<Ticket>,
    pub total_count: i32,
}

/// Data loaded for the ticket detail page.
#[derive(Debug, Clone)]
pub struct TicketDetailData {
    pub detail: GetTicketResponse,
    pub children: Vec<Ticket>,
    pub total_children: i32,
}

/// Data loaded for the flows page.
#[derive(Debug, Clone)]
pub struct FlowListData {
    pub workflows: Vec<WorkflowInfo>,
    pub total_count: i32,
}

/// Data loaded for the workers page.
#[derive(Debug, Clone)]
pub struct WorkerListData {
    pub workers: Vec<WorkerSummary>,
}

/// Data loaded for ticket activities.
#[derive(Debug, Clone)]
pub struct TicketActivitiesData {
    pub activities: Vec<ActivityEntry>,
}

/// Sub-model for the ticket activities page (full-screen activities viewer).
#[derive(Debug, Clone)]
pub struct TicketActivitiesModel {
    pub ticket_id: String,
    pub ticket_title: String,
    pub data: LoadState<TicketActivitiesData>,
    /// Selected row within the current page.
    pub selected_row: usize,
    /// Current page (client-side pagination).
    pub current_page: usize,
    /// Number of activities per page.
    pub page_size: usize,
    /// Unique authors extracted from the last successful fetch.
    /// Index 0 is always "all" (no filter).
    pub authors: Vec<String>,
    /// Currently selected author index (0 = all).
    pub author_index: usize,
}

/// Sub-model for the ticket body page (full-screen markdown body viewer).
#[derive(Debug)]
pub struct TicketBodyModel {
    pub ticket_id: String,
    pub title: String,
    pub body: String,
    /// Current vertical scroll offset (lines from the top).
    pub scroll_offset: usize,
    /// Height of the body pane from the last render, used for page scrolling.
    pub last_body_height: Cell<usize>,
    /// Rendered line count from the last render, used to clamp scroll.
    pub last_total_lines: Cell<usize>,
}

impl Clone for TicketBodyModel {
    fn clone(&self) -> Self {
        Self {
            ticket_id: self.ticket_id.clone(),
            title: self.title.clone(),
            body: self.body.clone(),
            scroll_offset: self.scroll_offset,
            last_body_height: self.last_body_height.clone(),
            last_total_lines: self.last_total_lines.clone(),
        }
    }
}

/// Tracks which tabs have dirty data from UI events and manages a cooldown
/// window so that rapid-fire events are batched into periodic fetches.
#[derive(Debug, Clone)]
pub struct UiEventThrottle {
    /// Tabs whose data has changed since the last flush.
    pub dirty: HashSet<TabId>,
    /// When the current cooldown window started, if one is active.
    pub cooldown_start: Option<Instant>,
}

impl UiEventThrottle {
    /// Create a new throttle with no dirty tabs and no active cooldown.
    pub fn new() -> Self {
        Self {
            dirty: HashSet::new(),
            cooldown_start: None,
        }
    }

    /// Mark the given tabs as dirty (their data has changed).
    pub fn mark_dirty(&mut self, tabs: impl IntoIterator<Item = TabId>) {
        self.dirty.extend(tabs);
    }

    /// Returns true if the cooldown has elapsed and there are dirty tabs
    /// waiting to be flushed.
    pub fn should_flush(&self) -> bool {
        if self.dirty.is_empty() {
            return false;
        }
        match self.cooldown_start {
            None => true,
            Some(start) => start.elapsed() >= THROTTLE_COOLDOWN,
        }
    }

    /// Drain all dirty tabs and restart the cooldown timer.
    ///
    /// Returns the set of tabs that were dirty. The caller is responsible
    /// for issuing re-fetch commands for those tabs.
    pub fn flush(&mut self) -> HashSet<TabId> {
        let tabs = std::mem::take(&mut self.dirty);
        if !tabs.is_empty() {
            self.cooldown_start = Some(Instant::now());
        }
        tabs
    }
}

/// Active banner notification state.
#[derive(Debug, Clone)]
pub struct BannerModel {
    /// The message text displayed in the banner.
    pub message: String,
    /// The visual variant (success/error) controlling colors.
    pub variant: BannerVariant,
    /// When the banner was created, used for auto-dismiss timing.
    pub created_at: Instant,
}

impl BannerModel {
    /// Returns true if this banner should be auto-dismissed based on elapsed time.
    /// Success banners expire after `BANNER_AUTO_DISMISS_SECS`; error banners are sticky.
    pub fn is_expired(&self) -> bool {
        match self.variant {
            BannerVariant::Success => {
                self.created_at.elapsed().as_secs() >= BANNER_AUTO_DISMISS_SECS
            }
            BannerVariant::Error => false,
        }
    }
}

/// Active status message state.
#[derive(Debug, Clone)]
pub struct StatusModel {
    /// The status text displayed in the header area.
    pub text: String,
}

/// Default page size for ticket tables (used when the render area height is
/// not yet known).
pub const TICKET_TABLE_DEFAULT_PAGE_SIZE: usize = 20;

/// Shared sub-model for ticket table state (reused on ticket list page and
/// ticket detail page for the children table).
///
/// Holds selection, pagination, and the raw ticket rows. The TicketTable
/// component renders from this slice; callers populate it with different
/// data sources (top-level tickets vs children of a parent).
#[derive(Debug, Clone)]
pub struct TicketTableModel {
    /// Ticket rows displayed on the current page.
    pub tickets: Vec<Ticket>,
    /// Total count of tickets matching the query (for pagination).
    pub total_count: i32,
    /// Selected row index within the current page.
    pub selected_row: usize,
    /// Current page (zero-indexed, server-side pagination).
    pub current_page: usize,
    /// Rows per page, derived from the render area height.
    pub page_size: usize,
}

impl TicketTableModel {
    /// Create a new, empty table model with default page size.
    pub fn empty() -> Self {
        Self {
            tickets: vec![],
            total_count: 0,
            selected_row: 0,
            current_page: 0,
            page_size: TICKET_TABLE_DEFAULT_PAGE_SIZE,
        }
    }

    /// Total number of pages given the current total_count and page_size.
    pub fn total_pages(&self) -> usize {
        let count = self.total_count as usize;
        if count == 0 || self.page_size == 0 {
            return 1;
        }
        count.div_ceil(self.page_size)
    }

    /// Navigate the selection up by one row, clamping at 0.
    pub fn navigate_up(&mut self) {
        if self.selected_row > 0 {
            self.selected_row -= 1;
        }
    }

    /// Navigate the selection down by one row, clamping at the last row.
    pub fn navigate_down(&mut self) {
        let count = self.tickets.len();
        if count > 0 && self.selected_row < count - 1 {
            self.selected_row += 1;
        }
    }

    /// Move to the previous page. Returns true if the page changed (caller
    /// should issue a fetch command).
    pub fn page_left(&mut self) -> bool {
        if self.current_page > 0 {
            self.current_page -= 1;
            self.selected_row = 0;
            true
        } else {
            false
        }
    }

    /// Move to the next page. Returns true if the page changed (caller
    /// should issue a fetch command).
    pub fn page_right(&mut self) -> bool {
        if self.current_page + 1 < self.total_pages() {
            self.current_page += 1;
            self.selected_row = 0;
            true
        } else {
            false
        }
    }

    /// Update page size based on render area height. Returns true if the
    /// page size changed (caller should issue a fetch command).
    pub fn update_page_size(&mut self, area_height: u16) -> bool {
        // 3 lines of chrome: 1 top border + 1 header row + 1 bottom border
        let chrome = 3u16;
        let available = area_height.saturating_sub(chrome) as usize;
        if available > 0 && available != self.page_size {
            self.page_size = available;
            self.current_page = 0;
            self.selected_row = 0;
            true
        } else {
            false
        }
    }

    /// Returns the currently selected ticket, if any.
    pub fn selected_ticket(&self) -> Option<&Ticket> {
        self.tickets.get(self.selected_row)
    }

    /// Page info string for the table footer (e.g. "Page 1/3 (42 total)").
    pub fn page_info(&self) -> String {
        let page_display = self.current_page + 1;
        let total_pages = self.total_pages();
        format!(
            "Page {}/{} ({} total)",
            page_display, total_pages, self.total_count
        )
    }
}

/// Sub-model for the ticket list page.
#[derive(Debug, Clone)]
pub struct TicketListModel {
    pub data: LoadState<TicketListData>,
}

/// Sub-model for the ticket detail page.
#[derive(Debug, Clone)]
pub struct TicketDetailModel {
    pub ticket_id: String,
    pub data: LoadState<TicketDetailData>,
    pub activities: LoadState<TicketActivitiesData>,
}

/// Sub-model for the flows page.
#[derive(Debug, Clone)]
pub struct FlowListModel {
    pub data: LoadState<FlowListData>,
}

/// Number of workers displayed per page.
pub const WORKER_PAGE_SIZE: usize = 20;

/// Sub-model for the workers page.
#[derive(Debug, Clone)]
pub struct WorkerListModel {
    pub data: LoadState<WorkerListData>,
    /// Selected row index within the current page.
    pub selected_row: usize,
    /// Current page (zero-indexed).
    pub current_page: usize,
}

/// Which overlay is currently active, if any.
#[derive(Debug, Clone)]
pub enum ActiveOverlay {
    /// Priority picker overlay for a specific ticket.
    PriorityPicker { ticket_id: String, cursor: usize },
    /// Filter menu overlay.
    FilterMenu {
        cursor: usize,
        expanded: Option<FilterCategory>,
        sub_cursor: usize,
    },
    /// Goto menu overlay with available targets.
    GotoMenu {
        targets: Vec<super::msg::GotoTarget>,
        cursor: usize,
    },
    /// Force-close confirmation overlay.
    ForceCloseConfirm {
        ticket_id: String,
        open_children: i32,
    },
    /// Create action menu overlay.
    CreateActionMenu {
        pending: super::msg::PendingTicket,
        cursor: usize,
    },
    /// Project input text overlay.
    ProjectInput { buffer: String },
    /// Settings overlay.
    Settings {
        level: SettingsLevel,
        top_cursor: usize,
        active_column: usize,
        column_cursors: [usize; 3],
        light_themes: Vec<String>,
        dark_themes: Vec<String>,
        custom_themes: Vec<String>,
    },
}

/// The filter categories available in the filter menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterCategory {
    Status,
    Priority,
    Project,
    ShowChildren,
}

/// Which level the settings overlay is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsLevel {
    /// Top-level settings menu.
    TopLevel,
    /// Theme picker with three columns.
    ThemePicker,
}

/// Persisted filter selections applied to the ticket list.
#[derive(Debug, Clone)]
pub struct TicketFilters {
    /// Which statuses are enabled. When empty, all are shown.
    pub statuses: Vec<String>,
    /// Which priorities are enabled. When empty, all are shown.
    pub priorities: Vec<i64>,
    /// Which projects are enabled. When empty, all are shown.
    pub projects: Vec<String>,
    /// Whether to show tickets that have a parent_id (children).
    pub show_children: bool,
}

impl Default for TicketFilters {
    fn default() -> Self {
        Self {
            statuses: vec!["open".to_string(), "in_progress".to_string()],
            priorities: vec![],
            projects: vec![],
            show_children: false,
        }
    }
}

/// The top-level application model for the v2 TEA architecture.
///
/// This struct holds all application state. It is owned by the main loop and
/// passed (by value) to the pure `update` function, which returns a new `Model`.
#[derive(Debug, Clone)]
pub struct Model {
    /// When true, the main loop should exit.
    pub should_quit: bool,
    /// Placeholder for future navigation state (active tab, page stack, etc.).
    pub navigation_model: NavigationModel,
    /// The input focus stack. Handlers are walked top-to-bottom on each key
    /// event; the first to capture wins. Also collects footer commands.
    pub input_stack: InputStack,
    /// Sub-model for the ticket list page.
    pub ticket_list: TicketListModel,
    /// Sub-model for the ticket detail page (set when viewing a ticket).
    pub ticket_detail: Option<TicketDetailModel>,
    /// Sub-model for the flows page.
    pub flow_list: FlowListModel,
    /// Sub-model for the workers page.
    pub worker_list: WorkerListModel,
    /// Throttle for UI event-driven data refreshes.
    pub ui_event_throttle: UiEventThrottle,
    /// Active banner notification, if any.
    pub banner: Option<BannerModel>,
    /// Active status message, if any.
    pub status: Option<StatusModel>,
    /// Currently active overlay, if any.
    pub active_overlay: Option<ActiveOverlay>,
    /// Ticket list filter state.
    pub ticket_filters: TicketFilters,
    /// Sub-model for the ticket activities page (set when viewing activities).
    pub ticket_activities: Option<TicketActivitiesModel>,
    /// Sub-model for the ticket body page (set when viewing body).
    pub ticket_body: Option<TicketBodyModel>,
    /// Notification tracking model for workflow state transitions.
    pub notifications: NotificationModel,
}

impl Model {
    /// Create the initial application model.
    pub fn initial() -> Self {
        let mut input_stack = InputStack::default();
        input_stack.push(Box::new(GlobalHandler));
        Self {
            should_quit: false,
            navigation_model: NavigationModel::initial(),
            input_stack,
            ticket_list: TicketListModel {
                data: LoadState::NotLoaded,
            },
            ticket_detail: None,
            flow_list: FlowListModel {
                data: LoadState::NotLoaded,
            },
            worker_list: WorkerListModel {
                data: LoadState::NotLoaded,
                selected_row: 0,
                current_page: 0,
            },
            ui_event_throttle: UiEventThrottle::new(),
            banner: None,
            status: None,
            active_overlay: None,
            ticket_filters: TicketFilters::default(),
            ticket_activities: None,
            ticket_body: None,
            notifications: NotificationModel::new(ur_config::NotificationConfig::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_model_does_not_quit() {
        let model = Model::initial();
        assert!(!model.should_quit);
    }

    #[test]
    fn load_state_not_loaded_is_not_loaded() {
        let state: LoadState<String> = LoadState::NotLoaded;
        assert!(!state.is_loaded());
        assert!(!state.is_loading());
        assert!(state.data().is_none());
    }

    #[test]
    fn load_state_loading_is_loading() {
        let state: LoadState<String> = LoadState::Loading;
        assert!(state.is_loading());
        assert!(!state.is_loaded());
        assert!(state.data().is_none());
    }

    #[test]
    fn load_state_loaded_has_data() {
        let state = LoadState::Loaded("hello".to_string());
        assert!(state.is_loaded());
        assert!(!state.is_loading());
        assert_eq!(state.data(), Some(&"hello".to_string()));
    }

    #[test]
    fn load_state_error_has_no_data() {
        let state: LoadState<String> = LoadState::Error("oops".to_string());
        assert!(!state.is_loaded());
        assert!(!state.is_loading());
        assert!(state.data().is_none());
    }

    #[test]
    fn initial_model_sub_models_not_loaded() {
        let model = Model::initial();
        assert!(matches!(model.ticket_list.data, LoadState::NotLoaded));
        assert!(matches!(model.flow_list.data, LoadState::NotLoaded));
        assert!(matches!(model.worker_list.data, LoadState::NotLoaded));
        assert!(model.ticket_detail.is_none());
    }

    #[test]
    fn throttle_new_has_no_dirty_tabs() {
        let throttle = UiEventThrottle::new();
        assert!(throttle.dirty.is_empty());
        assert!(throttle.cooldown_start.is_none());
    }

    #[test]
    fn throttle_mark_dirty_adds_tabs() {
        let mut throttle = UiEventThrottle::new();
        throttle.mark_dirty([TabId::Tickets, TabId::Flows]);
        assert!(throttle.dirty.contains(&TabId::Tickets));
        assert!(throttle.dirty.contains(&TabId::Flows));
    }

    #[test]
    fn throttle_should_flush_when_dirty_no_cooldown() {
        let mut throttle = UiEventThrottle::new();
        throttle.mark_dirty([TabId::Tickets]);
        assert!(throttle.should_flush());
    }

    #[test]
    fn throttle_should_not_flush_when_empty() {
        let throttle = UiEventThrottle::new();
        assert!(!throttle.should_flush());
    }

    #[test]
    fn throttle_should_not_flush_during_cooldown() {
        let mut throttle = UiEventThrottle::new();
        throttle.mark_dirty([TabId::Tickets]);
        throttle.cooldown_start = Some(Instant::now());
        assert!(!throttle.should_flush());
    }

    #[test]
    fn throttle_should_flush_after_cooldown_elapsed() {
        let mut throttle = UiEventThrottle::new();
        throttle.mark_dirty([TabId::Tickets]);
        throttle.cooldown_start = Some(Instant::now() - Duration::from_millis(300));
        assert!(throttle.should_flush());
    }

    #[test]
    fn throttle_flush_returns_dirty_and_clears() {
        let mut throttle = UiEventThrottle::new();
        throttle.mark_dirty([TabId::Tickets, TabId::Workers]);
        let flushed = throttle.flush();
        assert!(flushed.contains(&TabId::Tickets));
        assert!(flushed.contains(&TabId::Workers));
        assert!(throttle.dirty.is_empty());
        assert!(throttle.cooldown_start.is_some());
    }

    #[test]
    fn throttle_flush_empty_does_not_start_cooldown() {
        let mut throttle = UiEventThrottle::new();
        let flushed = throttle.flush();
        assert!(flushed.is_empty());
        assert!(throttle.cooldown_start.is_none());
    }

    #[test]
    fn throttle_accumulates_during_cooldown() {
        let mut throttle = UiEventThrottle::new();
        throttle.mark_dirty([TabId::Tickets]);
        let _ = throttle.flush();
        throttle.mark_dirty([TabId::Flows]);
        assert!(!throttle.should_flush());
        assert!(throttle.dirty.contains(&TabId::Flows));
    }

    #[test]
    fn initial_model_has_empty_throttle() {
        let model = Model::initial();
        assert!(model.ui_event_throttle.dirty.is_empty());
        assert!(model.ui_event_throttle.cooldown_start.is_none());
    }
}
