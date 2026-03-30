use crossterm::event::KeyEvent;
use ur_rpc::proto::core::WorkerSummary;
use ur_rpc::proto::ticket::{ActivityEntry, GetTicketResponse, Ticket, WorkflowInfo};

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
}
