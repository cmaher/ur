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
use crate::create_ticket as create_ticket_v1;
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
///
/// When a project filter is set, checks for a per-project theme override
/// in `projects.<key>.tui.theme` and uses it instead of the global theme.
fn build_tui_context(config: &ur_config::Config, project_filter: &Option<String>) -> TuiContext {
    let mut tui_config = config.tui.clone();

    // 5a: Per-project theme override at startup.
    if let Some(project_key) = project_filter
        && let Some(project_cfg) = config.projects.get(project_key)
        && let Some(tui) = &project_cfg.tui
        && let Some(theme_name) = &tui.theme_name
    {
        tui_config.theme_name = theme_name.clone();
    }

    let theme = Theme::resolve(&tui_config);
    let keymap = Keymap::default();
    let mut projects: Vec<String> = config.projects.keys().cloned().collect();
    projects.sort();

    TuiContext {
        theme,
        keymap,
        projects,
        project_configs: config.projects.clone(),
        tui_config,
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
    let mut ctx = build_tui_context(&config, &project_filter);

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Msg>();
    let cmd_runner = CmdRunner::new(
        msg_tx.clone(),
        port,
        project_filter.clone(),
        config.config_dir.clone(),
    );

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
    model.custom_theme_names = ctx.tui_config.custom_themes.keys().cloned().collect();
    model.custom_theme_names.sort();

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

        // Apply pending theme swap before rendering.
        if let Some(ref name) = model.pending_theme_swap {
            ctx.swap_theme(name);
            model.pending_theme_swap = None;
        }

        // Check if any command is SpawnEditor; if so, handle it specially.
        let editor_request = extract_editor_request(&cmds);
        let remaining = filter_non_editor_cmds(cmds);
        cmd_runner.execute_all(remaining);

        if let Some((parent_id, editor_project)) = editor_request {
            let resolved_project =
                resolve_editor_project(editor_project, parent_id.as_deref(), &project_filter, port)
                    .await;
            let pending = run_editor_flow(terminal, resolved_project, parent_id)?;
            if let Some(pending) = pending {
                let open_msg = Msg::Overlay(msg::OverlayMsg::OpenCreateActionMenu { pending });
                let (new_model, cmds) = update(model, open_msg);
                model = new_model;
                cmd_runner.execute_all(cmds);
            }
        }

        // Re-render after each update
        terminal.draw(|frame| view(&model, frame, &ctx))?;
    }

    Ok(())
}

/// Extract a SpawnEditor request from a list of commands, if present.
fn extract_editor_request(cmds: &[cmd::Cmd]) -> Option<(Option<String>, Option<String>)> {
    for c in cmds {
        if let cmd::Cmd::SpawnEditor { parent_id, project } = c {
            return Some((parent_id.clone(), project.clone()));
        }
        if let cmd::Cmd::Batch(batch) = c
            && let Some(result) = extract_editor_request(batch)
        {
            return Some(result);
        }
    }
    None
}

/// Filter out SpawnEditor commands from a list, returning only non-editor commands.
fn filter_non_editor_cmds(cmds: Vec<cmd::Cmd>) -> Vec<cmd::Cmd> {
    cmds.into_iter()
        .filter(|c| !matches!(c, cmd::Cmd::SpawnEditor { .. }))
        .map(|c| match c {
            cmd::Cmd::Batch(batch) => cmd::Cmd::Batch(filter_non_editor_cmds(batch)),
            other => other,
        })
        .collect()
}

/// Resolve the project for editor ticket creation.
///
/// Priority: explicit project from parent ticket > project_filter > None.
async fn resolve_editor_project(
    editor_project: Option<String>,
    parent_id: Option<&str>,
    project_filter: &Option<String>,
    port: u16,
) -> String {
    // If we have an explicit project (from parent), use it.
    if let Some(proj) = editor_project
        && !proj.is_empty()
    {
        return proj;
    }

    // Try to derive from parent ticket if we have a parent_id.
    if let Some(pid) = parent_id
        && let Ok(proj) = fetch_ticket_project(port, pid).await
        && !proj.is_empty()
    {
        return proj;
    }

    // Fall back to project filter.
    project_filter.clone().unwrap_or_default()
}

/// Fetch a ticket's project field from the server.
async fn fetch_ticket_project(port: u16, ticket_id: &str) -> Result<String, String> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::GetTicketRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    let channel = connect(port).await.map_err(|e| e.to_string())?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_string(),
            activity_author_filter: None,
        })
        .await
        .map_err(|e| e.to_string())?
        .into_inner();
    Ok(resp.ticket.map(|t| t.project).unwrap_or_default())
}

/// Run the editor flow: tear down terminal, spawn $EDITOR, parse result, re-init terminal.
///
/// Returns `Some(PendingTicket)` if the user wrote valid content, `None` if cancelled.
fn run_editor_flow(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    project: String,
    parent_id: Option<String>,
) -> anyhow::Result<Option<msg::PendingTicket>> {
    let template = create_ticket_v1::generate_template();
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("ur-ticket-{}.md", std::process::id()));
    std::fs::write(&tmp_path, &template)?;

    // Tear down terminal for editor.
    restore_terminal();

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor).arg(&tmp_path).status();

    // Re-initialize terminal regardless of editor outcome.
    reinit_terminal(terminal)?;

    let result = match status {
        Ok(exit) if exit.success() => {
            let content = std::fs::read_to_string(&tmp_path).unwrap_or_default();
            let _ = std::fs::remove_file(&tmp_path);
            create_ticket_v1::parse_ticket_file(&content).map(|parsed| msg::PendingTicket {
                project: if parsed.project.is_empty() {
                    project
                } else {
                    parsed.project
                },
                title: parsed.title,
                priority: parsed.priority,
                body: parsed.body,
                parent_id,
            })
        }
        _ => {
            let _ = std::fs::remove_file(&tmp_path);
            None
        }
    };

    Ok(result)
}

/// Re-initialize the terminal after returning from an external editor.
fn reinit_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(
        io::stdout(),
        crossterm::terminal::EnterAlternateScreen,
        crossterm::event::EnableMouseCapture
    )?;
    terminal.clear()?;
    Ok(())
}
