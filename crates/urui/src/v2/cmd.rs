/// Commands returned by the update function to be executed by the command runner.
///
/// Commands represent side effects: async operations, I/O, timers, etc.
/// The update function is pure — it returns `Cmd` values instead of performing
/// effects directly. The command runner executes them and feeds results back
/// as `Msg` variants.
#[derive(Debug)]
pub enum Cmd {
    /// No operation — used when update produces no side effects.
    None,
    /// Execute multiple commands concurrently.
    Batch(Vec<Cmd>),
    /// Request the application to quit.
    Quit,
    /// Fetch data from the server via gRPC.
    Fetch(FetchCmd),
    /// Subscribe to the UI event stream from the server.
    /// Spawns a long-lived background task that forwards server events as `Msg::UiEvent`.
    SubscribeUiEvents,
    /// Stop (kill) a worker by its ID.
    StopWorker { worker_id: String },
    /// Execute a ticket operation (dispatch, close, priority, create, etc.).
    TicketOp(super::msg::TicketOpMsg),
    /// Execute a flow operation (cancel, etc.).
    FlowOp(super::msg::FlowOpMsg),
    /// Execute a worker operation (kill, etc.).
    WorkerOp(super::msg::WorkerOpMsg),
    /// Fire a macOS desktop notification via terminal-notifier.
    FireDesktopNotification(super::notifications::DesktopNotification),
}

/// Data-fetching commands that trigger gRPC calls through the DataManager.
///
/// Each variant maps to a specific gRPC endpoint. The command runner spawns
/// an async task for each, and results arrive as `Msg::Data` variants.
#[derive(Debug, Clone)]
pub enum FetchCmd {
    /// Fetch the ticket list with optional pagination and status filters.
    Tickets {
        page_size: Option<i32>,
        offset: Option<i32>,
        include_children: Option<bool>,
        statuses: Vec<String>,
    },
    /// Fetch full ticket detail (ticket + children) by ID.
    TicketDetail {
        ticket_id: String,
        child_page_size: Option<i32>,
        child_offset: Option<i32>,
        child_status_filter: Option<String>,
    },
    /// Fetch workflows with optional pagination.
    Flows {
        page_size: Option<i32>,
        offset: Option<i32>,
    },
    /// Fetch all workers.
    Workers,
    /// Fetch activities for a specific ticket with optional author filter.
    Activities {
        ticket_id: String,
        author_filter: Option<String>,
    },
}

impl Cmd {
    /// Convenience: wrap multiple commands into a `Batch`, filtering out `None` variants.
    pub fn batch(cmds: Vec<Cmd>) -> Cmd {
        let filtered: Vec<Cmd> = cmds
            .into_iter()
            .filter(|c| !matches!(c, Cmd::None))
            .collect();
        match filtered.len() {
            0 => Cmd::None,
            1 => {
                // Unwrap is safe: we just checked length is 1
                filtered.into_iter().next().unwrap()
            }
            _ => Cmd::Batch(filtered),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_filters_none() {
        let cmd = Cmd::batch(vec![Cmd::None, Cmd::None]);
        assert!(matches!(cmd, Cmd::None));
    }

    #[test]
    fn batch_unwraps_single() {
        let cmd = Cmd::batch(vec![Cmd::None, Cmd::Quit]);
        assert!(matches!(cmd, Cmd::Quit));
    }

    #[test]
    fn batch_keeps_multiple() {
        let cmd = Cmd::batch(vec![Cmd::Quit, Cmd::Quit]);
        assert!(matches!(cmd, Cmd::Batch(_)));
    }

    #[test]
    fn fetch_tickets_cmd_is_debug() {
        let cmd = Cmd::Fetch(FetchCmd::Tickets {
            page_size: Some(50),
            offset: None,
            include_children: None,
            statuses: vec![],
        });
        let _ = format!("{cmd:?}");
    }

    #[test]
    fn fetch_cmd_in_batch() {
        let cmd = Cmd::batch(vec![
            Cmd::Fetch(FetchCmd::Workers),
            Cmd::Fetch(FetchCmd::Flows {
                page_size: None,
                offset: None,
            }),
        ]);
        assert!(matches!(cmd, Cmd::Batch(_)));
    }
}
