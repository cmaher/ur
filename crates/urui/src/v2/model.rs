use super::input::{GlobalHandler, InputStack};

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
}

/// Navigation state — tracks which page/tab is active.
///
/// Will be expanded with tab IDs, page stacks, etc. as navigation components
/// are implemented.
#[derive(Debug, Clone)]
pub struct NavigationModel {
    /// Placeholder field; will hold active tab, breadcrumb stack, etc.
    _placeholder: (),
}

impl Model {
    /// Create the initial application model.
    pub fn initial() -> Self {
        let mut input_stack = InputStack::default();
        input_stack.push(Box::new(GlobalHandler));
        Self {
            should_quit: false,
            navigation_model: NavigationModel { _placeholder: () },
            input_stack,
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
}
