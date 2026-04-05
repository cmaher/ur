mod cmd;
mod cmd_runner;
pub mod components;
mod context;
mod create_ticket;
pub mod input;
#[allow(dead_code)]
mod keymap;
mod model;
mod msg;
pub(crate) mod navigation;
mod notifications;
mod overlay_update;
pub mod pages;
mod terminal;
mod theme;
mod update;
mod view;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::Parser;
use crossterm::event::{self, Event};
use plugins::UiRegistry;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tokio::sync::mpsc;

use context::TuiContext;
use keymap::Keymap;
use terminal::{restore_terminal, setup_terminal};
use theme::Theme;

use cmd_runner::CmdRunner;
use model::Model;
use msg::Msg;
use update::update;
use view::view;

/// Polling interval for crossterm events.
const CROSSTERM_POLL_TIMEOUT: Duration = Duration::from_millis(50);
/// Tick interval for periodic UI housekeeping.
const TICK_INTERVAL: Duration = Duration::from_secs(5);

/// Terminal UI for the Ur coordination framework.
#[derive(Parser)]
#[command(name = "urui")]
struct Cli {
    /// Scope the UI to a single project key. If omitted, attempts to derive
    /// the project from the current directory name.
    #[arg(short = 'p', long = "project")]
    project: Option<String>,
}

/// Entry point for the urui TUI application.
///
/// Accepts a `UiRegistry` containing any registered UI plugins. Plugins are
/// configured during startup with their TOML tables from `config.plugins`.
/// An empty registry preserves all existing behaviour.
pub fn run(mut registry: UiRegistry) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let cli = Cli::parse();

        let mut terminal = setup_terminal()?;

        // Ensure terminal is restored even if the app panics.
        let default_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            default_hook(info);
        }));

        let config = ur_config::Config::load()?;

        // Configure all registered plugins with their TOML tables.
        registry.configure_all(&config.plugins)?;

        let result = tea_loop(&mut terminal, cli.project, &config).await;

        restore_terminal();

        result
    })
}

/// Poll for one crossterm event and send it through the channel.
/// Returns `true` if the loop should break (error or channel closed).
///
/// When `paused` is true, yields stdin to an external process (e.g. $EDITOR)
/// by sleeping instead of polling for events.
fn read_and_send_event(tx: &mpsc::UnboundedSender<Msg>, paused: &AtomicBool) -> bool {
    if paused.load(Ordering::Acquire) {
        std::thread::sleep(CROSSTERM_POLL_TIMEOUT);
        return false;
    }
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
    config: &ur_config::Config,
) -> anyhow::Result<()> {
    let port = config.server_port;
    let project_filter = ur_config::resolve_project(project, &config.projects);
    let mut ctx = build_tui_context(config, &project_filter);

    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel::<Msg>();
    let cmd_runner = CmdRunner::new(
        msg_tx.clone(),
        port,
        project_filter.clone(),
        config.config_dir.clone(),
    );

    // Pause flag: when true, the crossterm reader yields stdin to $EDITOR.
    let reader_paused = Arc::new(AtomicBool::new(false));

    // Spawn crossterm reader task
    let crossterm_tx = msg_tx.clone();
    let crossterm_paused = Arc::clone(&reader_paused);
    let _crossterm_handle = tokio::task::spawn_blocking(move || {
        while !crossterm_tx.is_closed() {
            if read_and_send_event(&crossterm_tx, &crossterm_paused) {
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

    // Trigger the initial data fetch for the starting tab so it doesn't
    // stay stuck on "Loading..." until the user manually refreshes.
    {
        let nav = std::mem::replace(
            &mut model.navigation_model,
            navigation::NavigationModel::initial(),
        );
        let init_cmds = nav.init_current_page(&mut model);
        model.navigation_model = nav;
        cmd_runner.execute_all(init_cmds);
    }

    // Initial render
    terminal.draw(|frame| view(&model, frame, &ctx))?;

    // Event loop
    let elc = EventLoopContext {
        project_filter: &project_filter,
        port,
        reader_paused: &reader_paused,
        cmd_runner: &cmd_runner,
    };
    run_event_loop(terminal, &mut msg_rx, model, &mut ctx, &elc).await
}

/// Shared state for the event loop, bundled to reduce argument counts.
struct EventLoopContext<'a> {
    project_filter: &'a Option<String>,
    port: u16,
    reader_paused: &'a Arc<AtomicBool>,
    cmd_runner: &'a CmdRunner,
}

/// Run the main event loop, processing messages until quit.
async fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    msg_rx: &mut mpsc::UnboundedReceiver<Msg>,
    mut model: Model,
    ctx: &mut TuiContext,
    elc: &EventLoopContext<'_>,
) -> anyhow::Result<()> {
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
        let edit_ticket_id = extract_edit_ticket_request(&cmds);
        let remaining = filter_non_editor_cmds(cmds);
        elc.cmd_runner.execute_all(remaining);

        if let Some(editor_req) = editor_request {
            model = handle_create_editor_cmd(
                terminal,
                model,
                editor_req,
                elc.project_filter,
                elc.port,
                elc.reader_paused,
                elc.cmd_runner,
            )
            .await?;
        }

        if let Some(ticket_id) = edit_ticket_id {
            model = handle_edit_ticket_cmd(
                terminal,
                model,
                &ticket_id,
                elc.port,
                elc.reader_paused,
                elc.cmd_runner,
            )
            .await;
        }

        // Re-render after each update
        terminal.draw(|frame| view(&model, frame, ctx))?;
    }

    Ok(())
}

/// Handle a SpawnEditor command: resolve project, open editor, show create action menu.
async fn handle_create_editor_cmd(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: Model,
    editor_req: EditorRequest,
    project_filter: &Option<String>,
    port: u16,
    reader_paused: &Arc<AtomicBool>,
    cmd_runner: &CmdRunner,
) -> anyhow::Result<Model> {
    let resolved_project = resolve_editor_project(
        editor_req.project,
        editor_req.parent_id.as_deref(),
        project_filter,
        port,
    )
    .await;
    reader_paused.store(true, Ordering::Release);
    let pending = run_editor_flow(
        terminal,
        resolved_project,
        editor_req.parent_id,
        editor_req.content,
    );
    reader_paused.store(false, Ordering::Release);
    let pending = pending?;
    if let Some(pending) = pending {
        let open_msg = Msg::Overlay(msg::OverlayMsg::OpenCreateActionMenu { pending });
        let (new_model, cmds) = update(model, open_msg);
        cmd_runner.execute_all(cmds);
        Ok(new_model)
    } else {
        Ok(model)
    }
}

/// Handle an EditTicket command: fetch ticket, open editor, submit update or show error.
async fn handle_edit_ticket_cmd(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    model: Model,
    ticket_id: &str,
    port: u16,
    reader_paused: &Arc<AtomicBool>,
    cmd_runner: &CmdRunner,
) -> Model {
    let edit_result = run_edit_ticket_flow(terminal, reader_paused, port, ticket_id).await;
    match edit_result {
        Ok(Some(op_msg)) => {
            let (new_model, cmds) = update(model, Msg::TicketOp(op_msg));
            cmd_runner.execute_all(cmds);
            new_model
        }
        Ok(None) => model,
        Err(e) => {
            let (new_model, cmds) = update(
                model,
                Msg::BannerShow {
                    message: format!("Edit failed: {e}"),
                    variant: components::banner::BannerVariant::Error,
                },
            );
            cmd_runner.execute_all(cmds);
            new_model
        }
    }
}

/// Extracted fields from a `SpawnEditor` command.
struct EditorRequest {
    parent_id: Option<String>,
    project: Option<String>,
    content: Option<String>,
}

/// Extract a SpawnEditor request from a list of commands, if present.
fn extract_editor_request(cmds: &[cmd::Cmd]) -> Option<EditorRequest> {
    for c in cmds {
        if let cmd::Cmd::SpawnEditor {
            parent_id,
            project,
            content,
        } = c
        {
            return Some(EditorRequest {
                parent_id: parent_id.clone(),
                project: project.clone(),
                content: content.clone(),
            });
        }
        if let cmd::Cmd::Batch(batch) = c
            && let Some(result) = extract_editor_request(batch)
        {
            return Some(result);
        }
    }
    None
}

/// Extract an EditTicket request from a list of commands, if present.
fn extract_edit_ticket_request(cmds: &[cmd::Cmd]) -> Option<String> {
    for c in cmds {
        if let cmd::Cmd::EditTicket { ticket_id } = c {
            return Some(ticket_id.clone());
        }
        if let cmd::Cmd::Batch(batch) = c
            && let Some(result) = extract_edit_ticket_request(batch)
        {
            return Some(result);
        }
    }
    None
}

/// Filter out SpawnEditor and EditTicket commands from a list, returning only other commands.
fn filter_non_editor_cmds(cmds: Vec<cmd::Cmd>) -> Vec<cmd::Cmd> {
    cmds.into_iter()
        .filter(|c| {
            !matches!(
                c,
                cmd::Cmd::SpawnEditor { .. } | cmd::Cmd::EditTicket { .. }
            )
        })
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
///
/// When `content` is `Some`, the editor opens with that pre-populated text instead of a
/// blank template (used by the Edit action to round-trip a PendingTicket through the editor).
fn run_editor_flow(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    project: String,
    parent_id: Option<String>,
    content: Option<String>,
) -> anyhow::Result<Option<msg::PendingTicket>> {
    let template = content.unwrap_or_else(create_ticket::generate_template);
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
            create_ticket::parse_ticket_file(&content).map(|parsed| msg::PendingTicket {
                project: if parsed.project.is_empty() {
                    project
                } else {
                    parsed.project
                },
                title: parsed.title,
                ticket_type: parsed.ticket_type,
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

/// Run the edit-ticket flow: fetch ticket, serialize to template, open $EDITOR, parse, return update op.
///
/// Returns `Some(TicketOpMsg::UpdateFields { .. })` if the user saved changes,
/// `None` if the user quit without saving.
async fn run_edit_ticket_flow(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    reader_paused: &Arc<AtomicBool>,
    port: u16,
    ticket_id: &str,
) -> anyhow::Result<Option<msg::TicketOpMsg>> {
    // 1. Fetch the ticket data.
    let ticket = fetch_ticket_for_edit(port, ticket_id).await?;

    // 2. Serialize to frontmatter template.
    let content = create_ticket::serialize_to_template(
        &ticket.project,
        &ticket.title,
        &ticket.ticket_type,
        ticket.priority,
        &ticket.body,
    );

    // 3. Open editor with pre-populated content.
    reader_paused.store(true, Ordering::Release);
    let result = run_edit_editor(terminal, &content);
    reader_paused.store(false, Ordering::Release);
    let parsed = result?;

    // 4. If user saved, build the update op.
    Ok(parsed.map(|p| msg::TicketOpMsg::UpdateFields {
        ticket_id: ticket_id.to_string(),
        project: if p.project.is_empty() {
            ticket.project
        } else {
            p.project
        },
        title: p.title,
        ticket_type: p.ticket_type,
        priority: p.priority,
        body: p.body,
    }))
}

/// Fetch a ticket's fields for editing.
async fn fetch_ticket_for_edit(
    port: u16,
    ticket_id: &str,
) -> anyhow::Result<create_ticket::PendingTicket> {
    use ur_rpc::connection::connect;
    use ur_rpc::proto::ticket::GetTicketRequest;
    use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

    let channel = connect(port).await?;
    let mut client = TicketServiceClient::new(channel);
    let resp = client
        .get_ticket(GetTicketRequest {
            id: ticket_id.to_string(),
            activity_author_filter: None,
        })
        .await?
        .into_inner();
    let ticket = resp
        .ticket
        .ok_or_else(|| anyhow::anyhow!("ticket {ticket_id} not found"))?;
    Ok(create_ticket::PendingTicket {
        project: ticket.project,
        title: ticket.title,
        ticket_type: ticket.ticket_type,
        priority: ticket.priority,
        body: ticket.body,
    })
}

/// Open $EDITOR with content, parse the result.
///
/// Returns `Some(PendingTicket)` if the user wrote valid content, `None` if cancelled.
fn run_edit_editor(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    content: &str,
) -> anyhow::Result<Option<create_ticket::PendingTicket>> {
    let tmp_dir = std::env::temp_dir();
    let tmp_path = tmp_dir.join(format!("ur-ticket-edit-{}.md", std::process::id()));
    std::fs::write(&tmp_path, content)?;

    // Tear down terminal for editor.
    restore_terminal();

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor).arg(&tmp_path).status();

    // Re-initialize terminal regardless of editor outcome.
    reinit_terminal(terminal)?;

    match status {
        Ok(exit) if exit.success() => {
            let new_content = std::fs::read_to_string(&tmp_path).unwrap_or_default();
            let _ = std::fs::remove_file(&tmp_path);
            // If the content is unchanged, treat as no-op.
            if new_content == content {
                return Ok(None);
            }
            Ok(create_ticket::parse_ticket_file(&new_content))
        }
        _ => {
            let _ = std::fs::remove_file(&tmp_path);
            Ok(None)
        }
    }
}

/// Re-initialize the terminal after returning from an external editor.
fn reinit_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> anyhow::Result<()> {
    crossterm::terminal::enable_raw_mode()?;
    crossterm::execute!(io::stdout(), crossterm::terminal::EnterAlternateScreen)?;
    terminal.clear()?;
    Ok(())
}
