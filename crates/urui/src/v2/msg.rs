use crossterm::event::KeyEvent;
use ur_rpc::proto::core::WorkerSummary;
use ur_rpc::proto::ticket::{ActivityEntry, GetTicketResponse, Ticket, WorkflowInfo};

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
}
