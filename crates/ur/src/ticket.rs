use anyhow::{Context, Result, bail};
use ticket_client::TicketArgs;
use tonic::transport::{Channel, Endpoint};
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

use crate::output::OutputManager;

async fn connect_ticket(port: u16) -> Result<TicketServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");
    let channel = Endpoint::try_from(addr)?
        .connect()
        .await
        .context("server is not running — run 'ur server start' first")?;
    Ok(TicketServiceClient::new(channel))
}

/// Resolve the project key for commands that require it.
///
/// Resolution order: explicit `--project/-p` flag → `UR_PROJECT` env → current directory name.
/// Returns an error if none resolves.
fn resolve_project(explicit: Option<String>) -> Result<String> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Ok(env_val) = std::env::var("UR_PROJECT")
        && !env_val.is_empty()
    {
        return Ok(env_val);
    }
    let cwd = std::env::current_dir().context("failed to get current working directory")?;
    let dir_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("cannot determine directory name from cwd"))?
        .to_owned();
    if dir_name.is_empty() {
        bail!("could not resolve project: no --project flag, UR_PROJECT env, or directory name");
    }
    Ok(dir_name)
}

/// Inject resolved project into ticket args that require it.
///
/// Commands taking a ticket ID (show, update, close, etc.) do not need project resolution —
/// the project comes from the stored ticket. Commands without a ticket ID (create, list,
/// dispatchable) require project context.
fn resolve_args_project(args: TicketArgs) -> Result<TicketArgs> {
    match args {
        TicketArgs::Create {
            title,
            project,
            ticket_type,
            parent,
            priority,
            body,
            wip,
        } => {
            let resolved = resolve_project(project)?;
            Ok(TicketArgs::Create {
                title,
                project: Some(resolved),
                ticket_type,
                parent,
                priority,
                body,
                wip,
            })
        }
        TicketArgs::List {
            project,
            all,
            epic,
            ticket_type,
            status,
            lifecycle,
        } => {
            let resolved = if all {
                None
            } else {
                Some(resolve_project(project)?)
            };
            Ok(TicketArgs::List {
                project: resolved,
                all,
                epic,
                ticket_type,
                status,
                lifecycle,
            })
        }
        TicketArgs::Dispatchable { epic_id, project } => {
            let resolved = resolve_project(project)?;
            Ok(TicketArgs::Dispatchable {
                epic_id,
                project: Some(resolved),
            })
        }
        other => Ok(other),
    }
}

pub async fn handle(port: u16, args: TicketArgs, output: &OutputManager) -> Result<()> {
    let args = resolve_args_project(args)?;
    let mut client = connect_ticket(port).await?;
    let result = ticket_client::execute(args, &mut client).await?;
    if output.is_json() {
        output.print_success(&result);
    } else {
        println!("{}", ticket_client::format_output(&result));
    }
    Ok(())
}
