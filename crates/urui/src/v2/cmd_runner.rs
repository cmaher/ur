use tokio::sync::mpsc;

use super::cmd::Cmd;
use super::msg::Msg;

/// Executes `Cmd` values produced by the update function and sends result
/// `Msg`s back through the event channel.
///
/// The command runner is the boundary between the pure TEA core and the
/// impure world of I/O and async operations.
#[derive(Clone)]
pub struct CmdRunner {
    msg_tx: mpsc::UnboundedSender<Msg>,
}

impl CmdRunner {
    /// Create a new command runner that sends result messages through the given channel.
    pub fn new(msg_tx: mpsc::UnboundedSender<Msg>) -> Self {
        Self { msg_tx }
    }

    /// Execute a command. Some commands (like `Quit`) produce messages synchronously;
    /// others will spawn async tasks that send messages when complete.
    pub fn execute(&self, cmd: Cmd) {
        match cmd {
            Cmd::None => {}
            Cmd::Quit => {
                // Send quit message back through the channel so the main loop exits.
                let _ = self.msg_tx.send(Msg::Quit);
            }
            Cmd::Batch(cmds) => {
                for cmd in cmds {
                    self.execute(cmd);
                }
            }
        }
    }

    /// Execute a list of commands.
    pub fn execute_all(&self, cmds: Vec<Cmd>) {
        for cmd in cmds {
            self.execute(cmd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn quit_cmd_sends_quit_msg() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner = CmdRunner::new(tx);

        runner.execute(Cmd::Quit);

        let msg = rx.recv().await.unwrap();
        assert!(matches!(msg, Msg::Quit));
    }

    #[tokio::test]
    async fn none_cmd_sends_nothing() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner = CmdRunner::new(tx);

        runner.execute(Cmd::None);
        drop(runner);

        // Channel should close with no messages
        assert!(rx.recv().await.is_none());
    }

    #[tokio::test]
    async fn batch_cmd_executes_all() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let runner = CmdRunner::new(tx);

        runner.execute(Cmd::Batch(vec![Cmd::Quit, Cmd::Quit]));
        drop(runner);

        let msg1 = rx.recv().await.unwrap();
        let msg2 = rx.recv().await.unwrap();
        assert!(matches!(msg1, Msg::Quit));
        assert!(matches!(msg2, Msg::Quit));
    }
}
