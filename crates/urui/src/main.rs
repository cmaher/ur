// Allow dead code during scaffold phase — items are used by upcoming modules
// (app, context, keymap, page, pages, theme, widgets).
#![allow(dead_code)]

// Modules with implementations:
mod data;
mod event;

// Planned modules — uncomment as implementations land:
// mod app;
mod context;
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

use crate::event::{AppEvent, EventManager, EventReceiver};

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

/// Minimal main loop that runs until the user presses 'q'.
///
/// This is a temporary implementation until `app.rs` provides the full `App` struct
/// with page routing, keymap resolution, and data fetching.
async fn run_minimal_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut receiver: EventReceiver,
    _config: &ur_config::Config,
) -> anyhow::Result<()> {
    use crossterm::event::KeyCode;
    use ratatui::style::{Color, Style};
    use ratatui::text::Line;
    use ratatui::widgets::Paragraph;

    loop {
        terminal.draw(|frame| {
            let area = frame.area();
            let text = Line::raw("urui — press q to quit");
            let paragraph =
                Paragraph::new(text).style(Style::default().fg(Color::White).bg(Color::Black));
            frame.render_widget(paragraph, area);
        })?;

        match receiver.recv().await {
            Some(AppEvent::Key(key)) => {
                if key.code == KeyCode::Char('q') {
                    break;
                }
            }
            Some(_) => {}
            None => break,
        }
    }

    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = ur_config::Config::load()?;

    // Verify server connectivity early so the user gets a clear error message.
    let _channel = ur_rpc::connection::connect(config.server_port).await?;

    let mut terminal = setup_terminal()?;

    // Ensure terminal is restored even if the app panics.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        default_hook(info);
    }));

    let (_event_manager, receiver) = EventManager::start();

    let result = run_minimal_loop(&mut terminal, receiver, &config).await;

    restore_terminal();

    result
}
