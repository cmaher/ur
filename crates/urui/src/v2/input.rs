use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::msg::{Msg, NavMsg};

/// A command displayed in the footer bar, collected from active input handlers.
#[derive(Debug, Clone)]
pub struct FooterCommand {
    /// Short label shown next to the key (e.g. "q").
    pub key_label: String,
    /// Human-readable description (e.g. "Quit").
    pub description: String,
    /// Whether this is a common command (rendered on the right side).
    pub common: bool,
}

/// Result of an input handler processing a key event.
#[derive(Debug)]
pub enum InputResult {
    /// The handler captured the key and produced a message.
    Capture(Msg),
    /// The handler did not handle this key; let the next handler try.
    Bubble,
}

/// Trait for components that can handle keyboard input.
///
/// Handlers are pushed onto the `InputStack` as components mount and popped
/// when they unmount. The stack is walked top-to-bottom on each key event;
/// the first handler to return `Capture` wins.
pub trait InputHandler {
    /// Attempt to handle a key event. Return `Capture(Msg)` to consume it,
    /// or `Bubble` to pass it to the next handler down the stack.
    fn handle_key(&self, key: KeyEvent) -> InputResult;

    /// Footer commands this handler wants to advertise.
    /// All active handlers contribute to the footer display.
    fn footer_commands(&self) -> Vec<FooterCommand>;

    /// A name for debugging / identification purposes.
    fn name(&self) -> &str;
}

/// A stack of input handlers walked top-to-bottom on each key event.
///
/// Handlers are pushed as components mount and popped when they unmount.
/// `dispatch()` walks from top (last pushed) to bottom (first pushed),
/// returning the first `Capture`. `footer_commands()` collects commands
/// from all active handlers (bottom to top, so global commands appear first).
#[derive(Default)]
pub struct InputStack {
    handlers: Vec<Box<dyn InputHandler>>,
}

impl std::fmt::Debug for InputStack {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.handlers.iter().map(|h| h.name()).collect();
        f.debug_struct("InputStack")
            .field("handlers", &names)
            .finish()
    }
}

impl Clone for InputStack {
    fn clone(&self) -> Self {
        // InputStack is not deeply cloneable because handlers are trait objects.
        // Model::clone() will get a fresh default stack. This is acceptable
        // because the stack is rebuilt from the model's component state.
        // In practice, the Model is only cloned in tests.
        Self::default()
    }
}

impl InputStack {
    /// Push a handler onto the top of the stack.
    pub fn push(&mut self, handler: Box<dyn InputHandler>) {
        self.handlers.push(handler);
    }

    /// Pop the topmost handler from the stack. Returns `None` if the stack is empty.
    pub fn pop(&mut self) -> Option<Box<dyn InputHandler>> {
        self.handlers.pop()
    }

    /// Dispatch a key event through the stack, top-to-bottom.
    /// Returns the `Msg` from the first handler that captures the key,
    /// or `None` if all handlers bubble.
    pub fn dispatch(&self, key: KeyEvent) -> Option<Msg> {
        for handler in self.handlers.iter().rev() {
            match handler.handle_key(key) {
                InputResult::Capture(msg) => return Some(msg),
                InputResult::Bubble => continue,
            }
        }
        None
    }

    /// Collect footer commands from all active handlers, bottom to top.
    /// Global handlers contribute their commands first, then more specific
    /// handlers layer on top.
    pub fn footer_commands(&self) -> Vec<FooterCommand> {
        let mut commands = Vec::new();
        for handler in &self.handlers {
            commands.extend(handler.footer_commands());
        }
        commands
    }

    /// Returns the number of handlers currently on the stack.
    pub fn len(&self) -> usize {
        self.handlers.len()
    }

    /// Returns true if the stack has no handlers.
    pub fn is_empty(&self) -> bool {
        self.handlers.is_empty()
    }
}

/// Global input handler that is always at the bottom of the stack.
///
/// Handles application-wide shortcuts:
/// - Ctrl+C → Quit
/// - Tab → switch to next tab (future: cycle through TabId variants)
pub struct GlobalHandler;

impl InputHandler for GlobalHandler {
    fn handle_key(&self, key: KeyEvent) -> InputResult {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return InputResult::Capture(Msg::Quit);
        }
        if key.code == KeyCode::Tab && key.modifiers == KeyModifiers::NONE {
            return InputResult::Capture(Msg::Nav(NavMsg::TabNext));
        }
        if key.code == KeyCode::Esc && key.modifiers == KeyModifiers::NONE {
            return InputResult::Capture(Msg::Nav(NavMsg::Pop));
        }
        InputResult::Bubble
    }

    fn footer_commands(&self) -> Vec<FooterCommand> {
        vec![
            FooterCommand {
                key_label: "Ctrl+C".to_string(),
                description: "Quit".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: "Tab".to_string(),
                description: "Switch tab".to_string(),
                common: true,
            },
            FooterCommand {
                key_label: "Esc".to_string(),
                description: "Back".to_string(),
                common: true,
            },
        ]
    }

    fn name(&self) -> &str {
        "global"
    }
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

    /// A test handler that captures a specific key and produces a specific message.
    struct TestHandler {
        capture_code: KeyCode,
        handler_name: &'static str,
    }

    impl InputHandler for TestHandler {
        fn handle_key(&self, key: KeyEvent) -> InputResult {
            if key.code == self.capture_code {
                InputResult::Capture(Msg::Quit)
            } else {
                InputResult::Bubble
            }
        }

        fn footer_commands(&self) -> Vec<FooterCommand> {
            vec![FooterCommand {
                key_label: format!("{:?}", self.capture_code),
                description: format!("{} action", self.handler_name),
                common: false,
            }]
        }

        fn name(&self) -> &str {
            self.handler_name
        }
    }

    #[test]
    fn empty_stack_dispatches_to_none() {
        let stack = InputStack::default();
        let key = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(stack.dispatch(key).is_none());
    }

    #[test]
    fn single_handler_captures() {
        let mut stack = InputStack::default();
        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('x'),
            handler_name: "test",
        }));

        let key = make_key(KeyCode::Char('x'), KeyModifiers::NONE);
        let result = stack.dispatch(key);
        assert!(result.is_some());
    }

    #[test]
    fn single_handler_bubbles() {
        let mut stack = InputStack::default();
        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('x'),
            handler_name: "test",
        }));

        let key = make_key(KeyCode::Char('y'), KeyModifiers::NONE);
        assert!(stack.dispatch(key).is_none());
    }

    #[test]
    fn top_handler_captures_before_bottom() {
        let mut stack = InputStack::default();
        // Bottom handler captures 'a'
        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('a'),
            handler_name: "bottom",
        }));
        // Top handler also captures 'a'
        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('a'),
            handler_name: "top",
        }));

        let key = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        let result = stack.dispatch(key);
        assert!(result.is_some());
        // The top handler should win (dispatch walks top-to-bottom)
    }

    #[test]
    fn capture_stops_bubbling() {
        let mut stack = InputStack::default();
        // Bottom captures 'b', top captures 'a'
        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('b'),
            handler_name: "bottom",
        }));
        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('a'),
            handler_name: "top",
        }));

        // 'a' should be captured by top, not reach bottom
        let key_a = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(stack.dispatch(key_a).is_some());

        // 'b' should bubble past top and be captured by bottom
        let key_b = make_key(KeyCode::Char('b'), KeyModifiers::NONE);
        assert!(stack.dispatch(key_b).is_some());

        // 'c' should bubble through both and return None
        let key_c = make_key(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(stack.dispatch(key_c).is_none());
    }

    #[test]
    fn footer_commands_collected_from_all_handlers() {
        let mut stack = InputStack::default();
        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('a'),
            handler_name: "first",
        }));
        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('b'),
            handler_name: "second",
        }));

        let commands = stack.footer_commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].description, "first action");
        assert_eq!(commands[1].description, "second action");
    }

    #[test]
    fn push_and_pop() {
        let mut stack = InputStack::default();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);

        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('a'),
            handler_name: "handler1",
        }));
        assert_eq!(stack.len(), 1);

        stack.push(Box::new(TestHandler {
            capture_code: KeyCode::Char('b'),
            handler_name: "handler2",
        }));
        assert_eq!(stack.len(), 2);

        let popped = stack.pop();
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().name(), "handler2");
        assert_eq!(stack.len(), 1);

        let popped = stack.pop();
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().name(), "handler1");
        assert!(stack.is_empty());

        assert!(stack.pop().is_none());
    }

    #[test]
    fn global_handler_captures_ctrl_c() {
        let handler = GlobalHandler;
        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        match handler.handle_key(key) {
            InputResult::Capture(msg) => assert!(matches!(msg, Msg::Quit)),
            InputResult::Bubble => panic!("expected Capture for Ctrl+C"),
        }
    }

    #[test]
    fn global_handler_bubbles_regular_keys() {
        let handler = GlobalHandler;
        let key = make_key(KeyCode::Char('a'), KeyModifiers::NONE);
        assert!(matches!(handler.handle_key(key), InputResult::Bubble));
    }

    #[test]
    fn global_handler_tab_produces_nav_tab_next() {
        let handler = GlobalHandler;
        let key = make_key(KeyCode::Tab, KeyModifiers::NONE);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::TabNext)) => {}
            other => panic!("expected Capture(Nav(TabNext)), got {other:?}"),
        }
    }

    #[test]
    fn global_handler_esc_produces_nav_pop() {
        let handler = GlobalHandler;
        let key = make_key(KeyCode::Esc, KeyModifiers::NONE);
        match handler.handle_key(key) {
            InputResult::Capture(Msg::Nav(NavMsg::Pop)) => {}
            other => panic!("expected Capture(Nav(Pop)), got {other:?}"),
        }
    }

    #[test]
    fn global_handler_footer_commands() {
        let handler = GlobalHandler;
        let commands = handler.footer_commands();
        assert!(commands.len() >= 3);
        assert!(commands.iter().all(|c| c.common));
    }

    #[test]
    fn global_handler_in_stack_captures_ctrl_c() {
        let mut stack = InputStack::default();
        stack.push(Box::new(GlobalHandler));

        let key = make_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let result = stack.dispatch(key);
        assert!(result.is_some());
        assert!(matches!(result.unwrap(), Msg::Quit));
    }

    #[test]
    fn input_stack_debug_shows_handler_names() {
        let mut stack = InputStack::default();
        stack.push(Box::new(GlobalHandler));
        let debug = format!("{stack:?}");
        assert!(debug.contains("global"));
    }
}
