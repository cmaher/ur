use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;

use crate::context::TuiContext;

use super::components::banner::render_banner;
use super::components::create_action_menu::render_create_action_menu;
use super::components::filter_menu::render_filter_menu;
use super::components::footer::render_footer;
use super::components::force_close_confirm::render_force_close_confirm;
use super::components::goto_menu::render_goto_menu;
use super::components::header::render_header;
use super::components::priority_picker::render_priority_picker;
use super::components::project_input::render_project_input;
use super::components::settings_overlay::render_settings_overlay;
use super::components::status::render_status;
use super::model::{ActiveOverlay, Model};

/// Root view function: renders the current model to the terminal frame.
///
/// In the TEA architecture, the view is a pure function from Model to UI.
/// It reads the model and draws widgets — no mutation, no side effects.
///
/// Layout:
/// - Row 0: Header (tab bar)
/// - Row 1: Sub-header (banner or status message, always reserved)
/// - Row 2..n-1: Content area (page-specific, future)
/// - Row n: Footer (commands from input stack)
pub fn view(model: &Model, frame: &mut Frame, ctx: &TuiContext) {
    let area = frame.area();

    // Fill the entire frame with the base background so no terminal
    // theme bleeds through in margins or empty regions.
    let base_style = Style::default()
        .bg(ctx.theme.base_100)
        .fg(ctx.theme.base_content);
    frame.buffer_mut().set_style(area, base_style);

    let chunks = Layout::vertical([
        Constraint::Length(1), // header
        Constraint::Length(1), // sub-header (banner or status, always reserved)
        Constraint::Fill(1),   // content
        Constraint::Length(1), // footer
    ])
    .split(area);

    // Header: tab bar
    render_header(chunks[0], frame.buffer_mut(), ctx, &model.navigation_model);

    // Sub-header: banner takes priority over status
    if let Some(ref banner) = model.banner {
        render_banner(chunks[1], frame.buffer_mut(), ctx, banner);
    } else if let Some(ref status) = model.status {
        render_status(chunks[1], frame.buffer_mut(), ctx, status);
    }

    // Content area: future page-specific rendering will go here.
    // For now this area is filled with the base background from above.

    // Active overlay (rendered on top of content area)
    render_active_overlay(area, frame.buffer_mut(), ctx, model);

    // Footer: commands collected from the input stack
    let commands = model.input_stack.footer_commands();
    render_footer(chunks[3], frame.buffer_mut(), ctx, &commands);
}

/// Render the currently active overlay, if any.
fn render_active_overlay(area: Rect, buf: &mut Buffer, ctx: &TuiContext, model: &Model) {
    match &model.active_overlay {
        None => {}
        Some(ActiveOverlay::PriorityPicker { .. }) => {
            render_priority_picker(area, buf, ctx, model);
        }
        Some(ActiveOverlay::FilterMenu { .. }) => {
            render_filter_menu(area, buf, ctx, model);
        }
        Some(ActiveOverlay::GotoMenu { .. }) => {
            render_goto_menu(area, buf, ctx, model);
        }
        Some(ActiveOverlay::ForceCloseConfirm { .. }) => {
            render_force_close_confirm(area, buf, ctx, model);
        }
        Some(ActiveOverlay::CreateActionMenu { .. }) => {
            render_create_action_menu(area, buf, ctx, model);
        }
        Some(ActiveOverlay::ProjectInput { .. }) => {
            render_project_input(area, buf, ctx, model);
        }
        Some(ActiveOverlay::Settings { .. }) => {
            render_settings_overlay(area, buf, ctx, model);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Keymap;
    use crate::theme::Theme;
    use ratatui::{Terminal, backend::TestBackend};
    use ur_config::TuiConfig;

    fn make_ctx() -> TuiContext {
        let tui_config = TuiConfig::default();
        let theme = Theme::resolve(&tui_config);
        let keymap = Keymap::default();
        TuiContext {
            theme,
            keymap,
            projects: vec![],
            project_configs: std::collections::HashMap::new(),
            tui_config: TuiConfig::default(),
            config_dir: std::path::PathBuf::from("/tmp/test-urui"),
            project_filter: None,
        }
    }

    #[test]
    fn view_renders_without_panic() {
        let model = Model::initial();
        let ctx = make_ctx();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| view(&model, frame, &ctx)).unwrap();
    }

    #[test]
    fn view_renders_header_with_tab_labels() {
        let model = Model::initial();
        let ctx = make_ctx();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| view(&model, frame, &ctx)).unwrap();

        let buf = terminal.backend().buffer();
        // Header is row 0 - check it contains tab text
        let row0: String = (0..80)
            .map(|x| {
                buf.cell((x, 0))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert!(row0.contains("ickets"), "header should show Tickets tab");
    }

    #[test]
    fn view_renders_footer_with_global_commands() {
        let model = Model::initial();
        let ctx = make_ctx();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| view(&model, frame, &ctx)).unwrap();

        let buf = terminal.backend().buffer();
        // Footer is the last row (row 23)
        let last_row: String = (0..80)
            .map(|x| {
                buf.cell((x, 23))
                    .unwrap()
                    .symbol()
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        // GlobalHandler provides Ctrl+C, Tab, Esc
        assert!(
            last_row.contains("Ctrl+C"),
            "footer should show Ctrl+C command"
        );
    }
}
