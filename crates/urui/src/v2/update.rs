use crossterm::event::{KeyCode, KeyModifiers};

use super::cmd::Cmd;
use super::model::Model;
use super::msg::Msg;

/// Pure update function: given the current model and a message, produces a new
/// model and a list of commands to execute.
///
/// This function must remain pure — no I/O, no async, no side effects. All
/// effects are expressed as `Cmd` values.
pub fn update(model: Model, msg: Msg) -> (Model, Vec<Cmd>) {
    match msg {
        Msg::Quit => {
            let mut model = model;
            model.should_quit = true;
            (model, vec![Cmd::Quit])
        }
        Msg::KeyPressed(key) => handle_key(model, key),
        Msg::Tick => (model, vec![]),
        Msg::CmdResult(_result) => {
            // Future: dispatch to sub-update functions based on result variant
            (model, vec![])
        }
    }
}

/// Handle a key press event. Ctrl+C triggers quit; other keys are no-ops for now.
fn handle_key(model: Model, key: crossterm::event::KeyEvent) -> (Model, Vec<Cmd>) {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
        return update(model, Msg::Quit);
    }
    (model, vec![])
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    use super::*;

    fn make_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn quit_message_sets_should_quit() {
        let model = Model::initial();
        let (new_model, cmds) = update(model, Msg::Quit);
        assert!(new_model.should_quit);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Quit)));
    }

    #[test]
    fn ctrl_c_triggers_quit() {
        let model = Model::initial();
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let (new_model, cmds) = update(model, Msg::KeyPressed(key));
        assert!(new_model.should_quit);
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Quit)));
    }

    #[test]
    fn tick_is_noop() {
        let model = Model::initial();
        let (new_model, cmds) = update(model, Msg::Tick);
        assert!(!new_model.should_quit);
        assert!(cmds.is_empty());
    }

    #[test]
    fn regular_key_is_noop() {
        let model = Model::initial();
        let key = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        let (new_model, cmds) = update(model, Msg::KeyPressed(key));
        assert!(!new_model.should_quit);
        assert!(cmds.is_empty());
    }
}
