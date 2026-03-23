// Allow dead code during scaffold phase — items are used by upcoming modules
// (app, context, keymap, page, pages, theme, widgets).
#![allow(dead_code)]

mod app;
mod context;
mod create_ticket;
mod data;
mod event;
mod keymap;
mod page;
mod pages;
mod terminal;
mod theme;
mod widgets;

use crate::app::App;
use crate::context::TuiContext;
use crate::data::DataManager;
use crate::event::EventManager;
use crate::keymap::Keymap;
use crate::terminal::{restore_terminal, setup_terminal};
use crate::theme::Theme;

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
        tui_config: config.tui.clone(),
        config_dir: config.config_dir.clone(),
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
    data_manager.subscribe_events();

    let mut app = App::new(ctx, data_manager, event_manager);
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
