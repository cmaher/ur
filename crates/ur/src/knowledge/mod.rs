pub mod args;
mod execute;
mod format;

pub use args::KnowledgeArgs;
pub use execute::execute;

use anyhow::{Context, Result};
use serde::Serialize;
use tonic::transport::{Channel, Endpoint};
use ur_rpc::proto::knowledge::knowledge_service_client::KnowledgeServiceClient;
use ur_rpc::proto::knowledge::{KnowledgeDoc, KnowledgeSummary};

use crate::output::OutputManager;

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KnowledgeOutput {
    Created { id: String },
    Read { doc: Box<KnowledgeDoc> },
    Updated { id: String },
    Deleted { id: String },
    Listed { docs: Vec<KnowledgeSummary> },
    Tags { tags: Vec<String> },
}

/// Format a `KnowledgeOutput` as human-readable text.
pub fn format_output(output: &KnowledgeOutput) -> String {
    match output {
        KnowledgeOutput::Created { id } => format!("Created {id}"),
        KnowledgeOutput::Read { doc } => format::format_doc(doc),
        KnowledgeOutput::Updated { id } => format!("Updated {id}"),
        KnowledgeOutput::Deleted { id } => format!("Deleted {id}"),
        KnowledgeOutput::Listed { docs } => {
            if docs.is_empty() {
                "No knowledge docs found.".to_string()
            } else {
                format::format_summary_list(docs)
            }
        }
        KnowledgeOutput::Tags { tags } => {
            if tags.is_empty() {
                "No tags found.".to_string()
            } else {
                format::format_tags(tags)
            }
        }
    }
}

async fn connect_knowledge(port: u16) -> Result<KnowledgeServiceClient<Channel>> {
    let addr = format!("http://127.0.0.1:{port}");
    let channel = Endpoint::try_from(addr)?
        .connect()
        .await
        .context("server is not running \u{2014} run 'ur server start' first")?;
    Ok(KnowledgeServiceClient::new(channel))
}

pub async fn handle(port: u16, args: KnowledgeArgs, output: &OutputManager) -> Result<()> {
    let mut client = connect_knowledge(port).await?;
    let result = execute(args, &mut client).await?;
    if output.is_json() {
        output.print_success(&result);
    } else {
        println!("{}", format_output(&result));
    }
    Ok(())
}
