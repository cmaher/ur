use crossterm::event::KeyEvent;

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
    /// A command completed and produced a result message.
    CmdResult(CmdResultMsg),
}

/// Result messages produced by asynchronous command execution.
///
/// As more features are added, variants will be added here for gRPC responses,
/// data fetches, etc.
#[derive(Debug, Clone)]
pub enum CmdResultMsg {
    /// Placeholder for future command results. Will be replaced by real
    /// variants as features are implemented.
    _Placeholder,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msg_is_debug() {
        let msg = Msg::Quit;
        // Verify Debug is implemented
        let _ = format!("{msg:?}");
    }

    #[test]
    fn msg_is_clone() {
        let msg = Msg::Quit;
        let _ = msg.clone();
    }
}
