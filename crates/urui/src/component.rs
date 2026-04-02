use ratatui::Frame;
use ratatui::layout::Rect;

use super::cmd::Cmd;
use super::input::InputHandler;
use super::model::Model;
use super::msg::Msg;

/// Trait for TEA components that participate in the navigation lifecycle.
///
/// Components are views over model slices — they do not own mutable state.
/// The model holds all data; `update` produces new state; components render
/// from it.
///
/// Lifecycle:
/// - `init` is called when the component is pushed onto a navigation stack.
///   It returns input handlers to push onto the input stack and initial
///   commands to execute (e.g. data fetches).
/// - `teardown` is called when the component is popped. It returns the
///   number of input handlers to pop from the stack.
/// - `update` processes messages relevant to this component.
/// - `render` draws the component into the given area.
pub trait Component {
    /// Called when this component is pushed onto the navigation stack.
    ///
    /// Returns a pair of:
    /// - Input handlers to push onto the input stack (in order; first element
    ///   is pushed first, so the last element ends up on top).
    /// - Commands to execute (e.g. initial data fetches).
    fn init(&self, model: &Model) -> (Vec<Box<dyn InputHandler>>, Vec<Cmd>);

    /// Called when this component is popped from the navigation stack.
    ///
    /// Returns the number of input handlers that were pushed during `init`
    /// and should now be popped from the input stack.
    fn teardown(&self, model: &Model) -> usize;

    /// Process a message relevant to this component.
    ///
    /// Returns an updated model and commands. The default implementation
    /// is a no-op passthrough.
    fn update(&self, model: Model, _msg: &Msg) -> (Model, Vec<Cmd>) {
        (model, vec![])
    }

    /// Render this component into the given area of the frame.
    fn render(&self, model: &Model, frame: &mut Frame, area: Rect);

    /// A human-readable name for this component (used in debugging).
    fn name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal test component to verify the trait is object-safe
    /// and can be implemented.
    struct TestComponent;

    impl Component for TestComponent {
        fn init(&self, _model: &Model) -> (Vec<Box<dyn InputHandler>>, Vec<Cmd>) {
            (vec![], vec![])
        }

        fn teardown(&self, _model: &Model) -> usize {
            0
        }

        fn render(&self, _model: &Model, _frame: &mut Frame, _area: Rect) {}

        fn name(&self) -> &str {
            "test"
        }
    }

    #[test]
    fn component_trait_is_implementable() {
        let comp = TestComponent;
        assert_eq!(comp.name(), "test");
    }

    #[test]
    fn default_update_is_passthrough() {
        let comp = TestComponent;
        let model = Model::initial();
        let msg = Msg::Tick;
        let (new_model, cmds) = comp.update(model.clone(), &msg);
        assert!(!new_model.should_quit);
        assert!(cmds.is_empty());
    }

    #[test]
    fn init_returns_empty_by_default() {
        let comp = TestComponent;
        let model = Model::initial();
        let (handlers, cmds) = comp.init(&model);
        assert!(handlers.is_empty());
        assert!(cmds.is_empty());
    }

    #[test]
    fn teardown_returns_zero() {
        let comp = TestComponent;
        let model = Model::initial();
        assert_eq!(comp.teardown(&model), 0);
    }
}
