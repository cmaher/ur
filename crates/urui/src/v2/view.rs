use ratatui::Frame;

use super::model::Model;

/// Root view function: renders the current model to the terminal frame.
///
/// In the TEA architecture, the view is a pure function from Model to UI.
/// It reads the model and draws widgets — no mutation, no side effects.
pub fn view(model: &Model, frame: &mut Frame) {
    let _area = frame.area();

    // For the initial scaffolding, render an empty screen.
    // Future: dispatch to page-specific view functions based on navigation_model.
    let _ = model;
}
