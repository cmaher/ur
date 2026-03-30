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
}
