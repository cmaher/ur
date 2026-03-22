// Allow dead code during scaffold phase — items are used by upcoming modules
// (app, context, keymap, page, pages, theme, widgets).
#![allow(dead_code)]

mod app;
mod context;
mod data;
mod event;
mod keymap;
mod page;
mod pages;
mod theme;
mod widgets;

use std::io;

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::app::App;
use crate::context::TuiContext;
use crate::data::DataManager;
use crate::event::EventManager;
use crate::keymap::Keymap;
use crate::theme::Theme;

/// Set up the terminal for TUI rendering: enable raw mode and enter alternate screen.
fn setup_terminal() -> anyhow::Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let terminal = Terminal::new(backend)?;
    Ok(terminal)
}

/// Restore the terminal to its original state: leave alternate screen and disable raw mode.
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = ur_config::Config::load()?;

    // Verify server connectivity early so the user gets a clear error message.
    let _channel = ur_rpc::connection::connect(config.server_port).await?;

    // Resolve theme and keymap from configuration.
    let theme = Theme::resolve(&config.tui);
    let keymap = resolve_keymap(&config.tui);
    let mut projects: Vec<String> = config.projects.keys().cloned().collect();
    projects.sort();
    let ctx = TuiContext {
        theme,
        keymap,
        projects,
        project_configs: config.projects.clone(),
    };

    let mut terminal = setup_terminal()?;

    // Ensure terminal is restored even if the app panics.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let (event_manager, receiver) = EventManager::start();
    let data_manager = DataManager::new(config.server_port, event_manager.sender());

    let mut app = App::new(ctx, data_manager);
    let result = app.run(&mut terminal, receiver).await;

    restore_terminal();

    result
}

/// Resolve the keymap from TUI configuration: use the named custom keymap if
/// one is configured and present, otherwise fall back to the default keymap.
fn resolve_keymap(tui: &ur_config::TuiConfig) -> Keymap {
    if tui.keymap_name != "default"
        && let Some(overrides) = tui.custom_keymaps.get(&tui.keymap_name)
    {
        return Keymap::from_config(overrides.clone());
    }
    Keymap::default()
}
