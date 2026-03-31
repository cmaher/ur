use crossterm::event::KeyEvent;
use ur_rpc::proto::core::WorkerSummary;
use ur_rpc::proto::ticket::{ActivityEntry, GetTicketResponse, Ticket, WorkflowInfo};

use super::components::banner::BannerVariant;
use super::navigation::{PageId, TabId};

/// Messages that drive the TEA update loop.
///
/// Every state change flows through a `Msg`. The update function pattern-matches
/// on these variants to produce a new `Model` and optional `Cmd`s.
#[derive(Debug, Clone)]
pub enum Msg {
    /// A keyboard event from the terminal.
    KeyPressed(KeyEvent),
    /// Periodic tick for UI housekeeping (e.g. cursor blink, status refresh).
    Tick,
    /// The user requested to quit (Ctrl+C or q).
    Quit,
    /// Asynchronous data fetched from the server arrived.
    Data(Box<DataMsg>),
    /// Navigation messages for tab switching and page stack manipulation.
    Nav(NavMsg),
    /// A batch of UI events received from the server's event stream.
    /// Each item describes an entity that changed (ticket, workflow, worker).
    UiEvent(Vec<UiEventItem>),
    /// Show a banner notification with the given message and variant.
    BannerShow {
        message: String,
        variant: BannerVariant,
    },
    /// Dismiss the currently active banner.
    BannerDismiss,
    /// Show a status message in the header area.
    StatusShow(String),
    /// Clear the current status message.
    StatusClear,
    /// Overlay messages for modal overlays.
    Overlay(OverlayMsg),
    /// A ticket operation request (user action → Cmd + status message).
    TicketOp(TicketOpMsg),
    /// A ticket operation result (gRPC completed → banner).
    TicketOpResult(TicketOpResultMsg),
    /// A flow operation request (user action → Cmd + status message).
    FlowOp(FlowOpMsg),
    /// A flow operation result (gRPC completed → banner).
    FlowOpResult(FlowOpResultMsg),
    /// A worker operation request (user action → Cmd + status message).
    WorkerOp(WorkerOpMsg),
    /// A worker operation result (gRPC completed → banner).
    WorkerOpResult(WorkerOpResultMsg),
}

/// Messages produced by overlay components.
#[derive(Debug, Clone)]
pub enum OverlayMsg {
    /// A key was captured by a modal overlay but has no meaningful action.
    /// The overlay stays open; this is a no-op for the update function.
    Consumed,
    /// Open the priority picker overlay for the given ticket, starting at
    /// the ticket's current priority.
    OpenPriorityPicker {
        ticket_id: String,
        current_priority: i64,
    },
    /// The user selected a priority in the picker.
    PrioritySelected { ticket_id: String, priority: i64 },
    /// Navigate the priority picker cursor by delta (+1 down, -1 up).
    PriorityPickerNavigate { delta: i32 },
    /// Confirm the currently highlighted priority.
    PriorityPickerConfirm,
    /// Quick-select a priority by digit (0-4).
    PriorityPickerQuickSelect { digit: i64 },
    /// The user cancelled the priority picker.
    PriorityCancelled,

    /// Open the filter menu overlay.
    OpenFilterMenu,
    /// Navigate the filter menu cursor.
    FilterMenuNavigate { delta: i32 },
    /// Activate/toggle the current item in the filter menu.
    FilterMenuActivate,
    /// Quick-toggle an item by digit key.
    FilterMenuQuickToggle { digit: char },
    /// The filter menu was closed (filters are mutated in-place in the model).
    FilterMenuClosed,

    /// Open the goto menu overlay with the given targets.
    OpenGotoMenu { targets: Vec<GotoTarget> },
    /// Navigate the goto menu cursor.
    GotoMenuNavigate { delta: i32 },
    /// Confirm the currently highlighted goto target.
    GotoMenuConfirm,
    /// Quick-select a goto target by digit key.
    GotoMenuQuickSelect { digit: usize },
    /// The user selected a goto target.
    GotoSelected(GotoTarget),
    /// The user cancelled the goto menu.
    GotoCancelled,

    /// Open the force-close confirmation overlay.
    OpenForceCloseConfirm {
        ticket_id: String,
        open_children: i32,
    },
    /// The user pressed y/1 to confirm force close (internal, resolves to ForceCloseConfirmed).
    ForceCloseConfirmYes,
    /// The user confirmed the force close.
    ForceCloseConfirmed { ticket_id: String },
    /// The user cancelled the force close.
    ForceCloseCancelled,

    /// Open the create action menu overlay.
    OpenCreateActionMenu { pending: PendingTicket },
    /// Navigate the create action menu cursor.
    CreateActionNavigate { delta: i32 },
    /// Confirm the currently highlighted create action.
    CreateActionConfirm,
    /// Quick-select a create action by digit key (1-4).
    CreateActionQuickSelect { index: usize },
    /// The user selected a create action.
    CreateActionSelected(CreateAction),

    /// Open the project input overlay.
    OpenProjectInput,
    /// A character was typed into the project input.
    ProjectInputChar(char),
    /// Backspace in the project input.
    ProjectInputBackspace,
    /// The user pressed Enter in the project input (resolved by update to ProjectInputSubmitted).
    ProjectInputSubmitRequest,
    /// The user submitted project input text.
    ProjectInputSubmitted(String),
    /// The user cancelled project input.
    ProjectInputCancelled,

    /// Open the settings overlay.
    OpenSettings { custom_theme_names: Vec<String> },
    /// Esc was pressed in the settings overlay (back or close depending on level).
    SettingsEsc,
    /// Navigate within the settings overlay.
    SettingsNavigate { direction: SettingsDirection },
    /// Activate the current settings item.
    SettingsActivate,
    /// The user selected a theme in settings.
    ThemeSelected(String),
    /// The settings overlay was closed.
    SettingsClosed,
}

/// A target that the user can navigate to via the goto menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GotoTarget {
    /// Display label shown in the menu (e.g., "Flow Details").
    pub label: String,
    /// The screen name to navigate to (e.g., "flow", "worker", "ticket").
    pub screen: String,
    /// The entity ID to navigate to.
    pub id: String,
}

/// The four actions available after editing a ticket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateAction {
    Create,
    Dispatch,
    Design,
    Abandon,
}

/// Direction for navigating within the settings overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsDirection {
    Up,
    Down,
    Left,
    Right,
}

/// Summary of a ticket pending creation.
#[derive(Debug, Clone)]
pub struct PendingTicket {
    pub project: String,
    pub title: String,
    pub priority: i64,
}

/// A single UI event received from the server's event stream.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UiEventItem {
    /// The entity type: "ticket", "workflow", or "worker".
    pub entity_type: String,
    /// The entity identifier (e.g. ticket ID or workflow ticket_id).
    pub entity_id: String,
}

/// Navigation messages for controlling tabs and page stacks.
#[derive(Debug, Clone)]
pub enum NavMsg {
    /// Switch to a specific tab. If already on that tab, pop to root.
    TabSwitch(TabId),
    /// Cycle to the next tab in display order.
    TabNext,
    /// Push a new page onto the active tab's stack.
    Push(PageId),
    /// Pop the current page from the active tab's stack.
    Pop,
    /// Navigate directly to a specific page (push if not already current).
    Goto(PageId),

    // ── Ticket table messages (shared across ticket list and detail) ──
    /// Navigate within the ticket table by delta (+1 down, -1 up).
    TicketTableNavigate { delta: i32 },
    /// Page right in the ticket table.
    TicketTablePageRight,
    /// Page left in the ticket table.
    TicketTablePageLeft,
    /// Select the currently highlighted ticket in the table.
    TicketTableSelect,

    // ── Ticket activities page messages ──────────────────────────────
    /// Navigate within the activities table by delta (+1 down, -1 up).
    ActivitiesNavigate { delta: i32 },
    /// Page right in the activities table.
    ActivitiesPageRight,
    /// Page left in the activities table.
    ActivitiesPageLeft,
    /// Cycle the author filter in the activities page.
    ActivitiesCycleFilter,
    /// Refresh the activities page data.
    ActivitiesRefresh,

    // ── Workers list page messages ────────────────────────────────────
    /// Navigate within the workers table by delta (+1 down, -1 up).
    WorkersNavigate { delta: i32 },
    /// Page right in the workers table.
    WorkersPageRight,
    /// Page left in the workers table.
    WorkersPageLeft,
    /// Refresh the workers list data.
    WorkersRefresh,
    /// Kill (stop) the currently selected worker.
    WorkersKill,
    /// Open the goto menu for the currently selected worker.
    WorkersGoto,

    // ── Ticket body page messages ────────────────────────────────────
    /// Scroll the body page down by one line.
    BodyScrollDown,
    /// Scroll the body page up by one line.
    BodyScrollUp,
    /// Page down in the body page.
    BodyPageDown,
    /// Page up in the body page.
    BodyPageUp,
}

/// Messages carrying data fetched asynchronously from gRPC calls.
///
/// Each variant corresponds to a `FetchCmd` and carries either the
/// successfully loaded data or an error string.
#[derive(Debug, Clone)]
pub enum DataMsg {
    /// Ticket list fetched: (tickets, total_count).
    TicketsLoaded(Result<(Vec<Ticket>, i32), String>),
    /// Full ticket detail fetched: (detail_response, children, total_children).
    DetailLoaded(Box<Result<(GetTicketResponse, Vec<Ticket>, i32), String>>),
    /// Workflow list fetched: (workflows, total_count).
    FlowsLoaded(Result<(Vec<WorkflowInfo>, i32), String>),
    /// Worker list fetched.
    WorkersLoaded(Result<Vec<WorkerSummary>, String>),
    /// Activities for a specific ticket fetched.
    ActivitiesLoaded {
        ticket_id: String,
        result: Result<Vec<ActivityEntry>, String>,
    },
    /// A worker stop (kill) operation completed.
    WorkerStopped {
        worker_id: String,
        result: Result<(), String>,
    },
}

/// Ticket operation request messages. Each variant carries the parameters needed
/// to initiate the operation. The update function returns a `Cmd::TicketOp` and
/// sets a status message while the operation is in flight.
#[derive(Debug, Clone)]
pub enum TicketOpMsg {
    /// Dispatch a single ticket (create workflow + launch worker).
    Dispatch {
        ticket_id: String,
        project_key: String,
        image_id: String,
    },
    /// Dispatch the parent ticket from a detail view (same RPC as Dispatch,
    /// but targets the parent rather than a selected child).
    DispatchAll {
        ticket_id: String,
        project_key: String,
        image_id: String,
    },
    /// Close a ticket by setting its status to "closed".
    Close { ticket_id: String },
    /// Force-close a ticket and all its open children.
    ForceClose { ticket_id: String },
    /// Set a ticket's priority.
    SetPriority { ticket_id: String, priority: i64 },
    /// Create a new ticket from a pending ticket template.
    Create { pending: PendingTicket },
    /// Create a ticket and immediately dispatch it.
    CreateAndDispatch {
        pending: PendingTicket,
        project_key: String,
        image_id: String,
    },
    /// Create a ticket and launch a design worker for it.
    CreateAndDesign {
        pending: PendingTicket,
        project_key: String,
        image_id: String,
    },
    /// Launch a design worker for an existing ticket.
    LaunchDesign {
        ticket_id: String,
        project_key: String,
        image_id: String,
    },
    /// Redrive a ticket's workflow to verifying status.
    Redrive { ticket_id: String },
}

/// Ticket operation result messages. Each variant carries the outcome of a
/// completed gRPC call. The update function clears the status and shows a banner.
#[derive(Debug, Clone)]
pub enum TicketOpResultMsg {
    /// Dispatch completed.
    Dispatched { result: Result<String, String> },
    /// Close completed.
    Closed { result: Result<String, String> },
    /// Force-close completed.
    ForceClosed { result: Result<String, String> },
    /// Priority set completed.
    PrioritySet { result: Result<String, String> },
    /// Ticket created.
    Created { result: Result<String, String> },
    /// Design worker launched.
    DesignLaunched { result: Result<String, String> },
    /// Redrive completed.
    Redriven { result: Result<String, String> },
}

/// Flow operation request messages. Each variant carries the parameters needed
/// to initiate the operation. The update function returns a `Cmd::FlowOp` and
/// sets a status message while the operation is in flight.
#[derive(Debug, Clone)]
pub enum FlowOpMsg {
    /// Cancel the active workflow for a ticket.
    Cancel { ticket_id: String },
}

/// Flow operation result messages. Each variant carries the outcome of a
/// completed gRPC call. The update function clears the status and shows a banner.
#[derive(Debug, Clone)]
pub enum FlowOpResultMsg {
    /// Cancel completed.
    Cancelled { result: Result<String, String> },
}

/// Worker operation request messages. Each variant carries the parameters needed
/// to initiate the operation. The update function returns a `Cmd::WorkerOp` and
/// sets a status message while the operation is in flight.
#[derive(Debug, Clone)]
pub enum WorkerOpMsg {
    /// Kill (stop) a worker by its ID.
    Kill { worker_id: String },
}

/// Worker operation result messages. Each variant carries the outcome of a
/// completed gRPC call. The update function clears the status and shows a banner.
#[derive(Debug, Clone)]
pub enum WorkerOpResultMsg {
    /// Kill completed.
    Killed { result: Result<String, String> },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_is_debug() {
        let msg = Msg::Quit;
        let _ = format!("{msg:?}");
    }

    #[test]
    fn msg_is_clone() {
        let msg = Msg::Quit;
        let _ = msg.clone();
    }

    #[test]
    fn data_msg_is_debug() {
        let msg = DataMsg::WorkersLoaded(Ok(vec![]));
        let _ = format!("{msg:?}");
    }

    #[test]
    fn data_msg_tickets_error() {
        let msg = DataMsg::TicketsLoaded(Err("connection refused".to_string()));
        let _ = format!("{msg:?}");
    }

    #[test]
    fn nav_msg_is_debug() {
        let msg = NavMsg::TabSwitch(TabId::Tickets);
        let _ = format!("{msg:?}");
    }

    #[test]
    fn nav_msg_is_clone() {
        let msg = NavMsg::Pop;
        let _ = msg.clone();
    }

    #[test]
    fn msg_nav_variant() {
        let msg = Msg::Nav(NavMsg::Push(PageId::TicketList));
        let _ = format!("{msg:?}");
    }

    #[test]
    fn ui_event_item_is_debug_clone() {
        let item = UiEventItem {
            entity_type: "ticket".to_string(),
            entity_id: "ur-abc".to_string(),
        };
        let _ = format!("{item:?}");
        let _ = item.clone();
    }

    #[test]
    fn msg_ui_event_variant() {
        let msg = Msg::UiEvent(vec![UiEventItem {
            entity_type: "workflow".to_string(),
            entity_id: "ur-xyz".to_string(),
        }]);
        let _ = format!("{msg:?}");
    }
}
