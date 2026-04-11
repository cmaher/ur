pub mod args;
mod execute;

pub use args::FlowArgs;
pub use execute::execute;

use anyhow::{Context, Result};
use serde::Serialize;
use tonic::transport::Channel;
use ur_rpc::proto::ticket::WorkflowInfo;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

use crate::output::OutputManager;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FlowOutput {
    Shown {
        workflow: Box<WorkflowInfo>,
    },
    Listed {
        workflows: Vec<WorkflowInfo>,
    },
    Cancelled {
        ticket_id: String,
    },
    NoverifySet {
        id: String,
    },
    Redriven {
        id: String,
        lifecycle_status: String,
    },
    AutoapproveSet {
        id: String,
        feedback_mode: String,
    },
}

/// Format a `FlowOutput` as human-readable text.
pub fn format_output(output: &FlowOutput) -> String {
    match output {
        FlowOutput::Shown { workflow } => format_workflow(workflow),
        FlowOutput::Listed { workflows } => {
            if workflows.is_empty() {
                "No workflows found.".to_string()
            } else {
                workflows
                    .iter()
                    .map(format_workflow_line)
                    .collect::<Vec<_>>()
                    .join("\n")
            }
        }
        FlowOutput::Cancelled { ticket_id } => {
            format!("Cancelled workflow for {ticket_id}")
        }
        FlowOutput::NoverifySet { id } => {
            format!("Set noverify on {id}")
        }
        FlowOutput::Redriven {
            id,
            lifecycle_status,
        } => {
            format!("Redrove {id} to {lifecycle_status}")
        }
        FlowOutput::AutoapproveSet { id, feedback_mode } => {
            format!("Set autoapprove on {id} (feedback_mode={feedback_mode})")
        }
    }
}

fn format_workflow(w: &WorkflowInfo) -> String {
    let mut lines = vec![
        format!("Workflow:  {}", w.id),
        format!("Ticket:   {}", w.ticket_id),
        format!("Status:   {}", w.status),
    ];
    if w.stalled {
        lines.push(format!("Stalled:  yes ({})", w.stall_reason));
    }
    if w.implement_cycles > 0 {
        lines.push(format!("Cycles:   {}", w.implement_cycles));
    }
    if !w.worker_id.is_empty() {
        lines.push(format!("Worker:   {}", w.worker_id));
    }
    if !w.feedback_mode.is_empty() {
        lines.push(format!("Feedback: {}", w.feedback_mode));
    }
    if !w.pr_url.is_empty() {
        lines.push(format!("PR:       {}", w.pr_url));
    }
    if !w.created_at.is_empty() {
        lines.push(format!("Created:  {}", w.created_at));
    }
    lines.join("\n")
}

fn format_workflow_line(w: &WorkflowInfo) -> String {
    let stall_marker = if w.stalled { " [STALLED]" } else { "" };
    let link = if w.pr_url.is_empty() {
        &w.id
    } else {
        &w.pr_url
    };
    format!(
        "{} {} {} cycles={}{}",
        w.ticket_id, link, w.status, w.implement_cycles, stall_marker
    )
}

async fn connect_flow(port: u16) -> Result<TicketServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");
    let retry_channel =
        ur_rpc::retry::RetryChannel::new(&addr, ur_rpc::retry::RetryConfig::default())
            .context("invalid server address")?;
    Ok(TicketServiceClient::new(retry_channel.channel().clone()))
}

pub async fn handle(port: u16, args: FlowArgs, output: &OutputManager) -> Result<()> {
    let mut client = connect_flow(port).await?;
    let result = execute(args, &mut client).await?;
    if output.is_json() {
        output.print_success(&result);
    } else {
        println!("{}", format_output(&result));
    }
    Ok(())
}
