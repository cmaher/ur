mod cmd;
mod cmd_runner;
mod component;
pub mod components;
mod create_ticket;
pub mod input;
mod model;
mod msg;
pub(crate) mod navigation;
mod notifications;
mod overlay_update;
pub mod pages;
mod update;
mod view;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use crate::context::TuiContext;
use crate::keymap::Keymap;
use crate::terminal::{restore_terminal, setup_terminal};
use crate::theme::Theme;

use self::cmd_runner::CmdRunner;
use self::model::Model;
use self::msg::Msg;
use self::update::update;
use self::view::view;

/// Polling interval for crossterm events.
const CROSSTERM_POLL_TIMEOUT: Duration = Duration::from_millis(50);
/// Tick interval for periodic UI housekeeping.
const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Entry point for the v2 TEA-based UI.
///
/// Sets up the terminal, runs the TEA loop (model-update-view), and restores
/// the terminal on exit. All state lives in `Model`; all mutations flow through
/// the pure `update` function; side effects are expressed as `Cmd` values
/// executed by the `CmdRunner`.
pub async fn run_v2(project: Option<String>) -> anyhow::Result<()> {
    let mut terminal = setup_terminal()?;

    // Ensure terminal is restored even if the app panics.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let result = tea_loop(&mut terminal, project).await;

    restore_terminal();

    result
}

/// Poll for one crossterm event and send it through the channel.
/// Returns `true` if the loop should break (error or channel closed).
fn read_and_send_event(tx: &mpsc::UnboundedSender<Msg>) -> bool {
    match event::poll(CROSSTERM_POLL_TIMEOUT) {
        Ok(true) => match event::read() {
            Ok(Event::Key(key)) => tx.send(Msg::KeyPressed(key)).is_err(),
            Ok(_) => false,
            Err(_) => true,
        },
        Ok(false) => false,
        Err(_) => true,
    }
}

/// Build a `TuiContext` from the loaded configuration.
fn build_tui_context(config: &ur_config::Config, project_filter: &Option<String>) -> TuiContext {
    let theme = Theme::resolve(&config.tui);
    let keymap = Keymap::default();
    let mut projects: Vec<String> = config.projects.keys().cloned().collect();
    projects.sort();

    TuiContext {
        theme,
        keymap,
        projects,
        project_configs: config.projects.clone(),
        tui_config: config.tui.clone(),
        config_dir: config.config_dir.clone(),
        project_filter: project_filter.clone(),
    }
}

/// The core TEA loop: read events, update model, execute commands, render view.
async fn tea_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    project: Option<String>,
) -> anyhow::Result<()> {
    let config = ur_config::Config::load()?;
    let port = config.server_port;
    let project_filter = ur_config::resolve_project(project, &config.projects);
    let ctx = build_tui_context(&config, &project_filter);

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Msg>();
    let cmd_runner = CmdRunner::new(msg_tx.clone(), port, project_filter);

    // Spawn crossterm reader task
    let crossterm_tx = msg_tx.clone();
    let _crossterm_handle = tokio::task::spawn_blocking(move || {
        while !crossterm_tx.is_closed() {
            if read_and_send_event(&crossterm_tx) {
                break;
            }
        }
    });

    // Spawn tick timer task
    let tick_tx = msg_tx;
    let _tick_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(TICK_INTERVAL);
        interval.tick().await; // skip first immediate tick
        loop {
            interval.tick().await;
            if tick_tx.send(Msg::Tick).is_err() {
                break;
            }
        }
    });

    let mut model = Model::initial();
    model.notifications = notifications::NotificationModel::new(config.tui.notifications.clone());

    // Subscribe to the server's UI event stream for live updates.
    cmd_runner.execute(cmd::Cmd::SubscribeUiEvents);

    // Initial render
    terminal.draw(|frame| view(&model, frame, &ctx))?;

    // Event loop
    while let Some(msg) = msg_rx.recv().await {
        let (new_model, cmds) = update(model, msg);
        model = new_model;

        if model.should_quit {
            break;
        }

        // Execute commands (may produce new messages)
        cmd_runner.execute_all(cmds);

        // Re-render after each update
        terminal.draw(|frame| view(&model, frame, &ctx))?;
    }

    Ok(())
}
