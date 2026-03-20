use anyhow::{Context, Result};
use ur_rpc::error::StatusResultExt;
use ur_rpc::proto::knowledge::knowledge_service_client::KnowledgeServiceClient;
use ur_rpc::proto::knowledge::*;

use super::KnowledgeOutput;
use super::args::KnowledgeArgs;

/// Execute a knowledge subcommand against the given gRPC client.
pub async fn execute<T>(
    args: KnowledgeArgs,
    client: &mut KnowledgeServiceClient<T>,
) -> Result<KnowledgeOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    match args {
        KnowledgeArgs::Create {
            title,
            project,
            shared,
            description,
            body,
            tags,
        } => execute_create(client, title, project, shared, description, body, tags).await,
        KnowledgeArgs::Read { id } => execute_read(client, id).await,
        KnowledgeArgs::Update {
            id,
            title,
            description,
            body,
            tags,
        } => execute_update(client, id, title, description, body, tags).await,
        KnowledgeArgs::Delete { id } => execute_delete(client, id).await,
        KnowledgeArgs::List {
            project,
            shared,
            tag,
        } => execute_list(client, project, shared, tag).await,
        KnowledgeArgs::ListTags { project, shared } => {
            execute_list_tags(client, project, shared).await
        }
    }
}

/// Build the content string from optional description and body.
fn build_content(description: Option<String>, body: &str) -> String {
    match description {
        Some(desc) if !body.is_empty() => format!("{desc}\n\n{body}"),
        Some(desc) => desc,
        None => body.to_owned(),
    }
}

/// Resolve the source field from --project / --shared flags.
fn resolve_source(project: Option<String>, shared: bool) -> String {
    if shared {
        "shared".to_owned()
    } else {
        project.unwrap_or_default()
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_create<T>(
    client: &mut KnowledgeServiceClient<T>,
    title: String,
    project: Option<String>,
    shared: bool,
    description: Option<String>,
    body: String,
    tags: Vec<String>,
) -> Result<KnowledgeOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let source = resolve_source(project, shared);
    let content = build_content(description, &body);
    let resp = client
        .create_knowledge(CreateKnowledgeRequest {
            title,
            content,
            source,
            tags,
        })
        .await
        .with_status_context("create knowledge")?;
    let id = resp.into_inner().id;
    Ok(KnowledgeOutput::Created { id })
}

async fn execute_read<T>(
    client: &mut KnowledgeServiceClient<T>,
    id: String,
) -> Result<KnowledgeOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let resp = client
        .get_knowledge(GetKnowledgeRequest { id: id.clone() })
        .await
        .with_status_context("get knowledge")?;
    let doc = resp
        .into_inner()
        .doc
        .context("server returned empty knowledge doc")?;
    Ok(KnowledgeOutput::Read { doc: Box::new(doc) })
}

async fn execute_update<T>(
    client: &mut KnowledgeServiceClient<T>,
    id: String,
    title: Option<String>,
    description: Option<String>,
    body: Option<String>,
    tags: Vec<String>,
) -> Result<KnowledgeOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    // Build content from description/body if either is provided
    let content = match (&description, &body) {
        (Some(desc), Some(b)) => Some(build_content(Some(desc.clone()), b)),
        (Some(desc), None) => Some(desc.clone()),
        (None, Some(b)) => Some(b.clone()),
        (None, None) => None,
    };

    let update_tags = !tags.is_empty();

    client
        .update_knowledge(UpdateKnowledgeRequest {
            id: id.clone(),
            title,
            content,
            source: None,
            tags,
            update_tags,
        })
        .await
        .with_status_context("update knowledge")?;
    Ok(KnowledgeOutput::Updated { id })
}

async fn execute_delete<T>(
    client: &mut KnowledgeServiceClient<T>,
    id: String,
) -> Result<KnowledgeOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    client
        .delete_knowledge(DeleteKnowledgeRequest { id: id.clone() })
        .await
        .with_status_context("delete knowledge")?;
    Ok(KnowledgeOutput::Deleted { id })
}

async fn execute_list<T>(
    client: &mut KnowledgeServiceClient<T>,
    project: Option<String>,
    shared: bool,
    tag: Option<String>,
) -> Result<KnowledgeOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let source = Some(resolve_source(project, shared));
    let resp = client
        .list_knowledge(ListKnowledgeRequest { tag, source })
        .await
        .with_status_context("list knowledge")?;
    let docs = resp.into_inner().docs;
    Ok(KnowledgeOutput::Listed { docs })
}

async fn execute_list_tags<T>(
    client: &mut KnowledgeServiceClient<T>,
    project: Option<String>,
    shared: bool,
) -> Result<KnowledgeOutput>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    // The ListTags RPC doesn't have a source filter, so we list all tags
    // and rely on server-side filtering if available in future versions.
    let _ = (project, shared);
    let resp = client
        .list_tags(ListTagsRequest {})
        .await
        .with_status_context("list knowledge tags")?;
    let tags = resp.into_inner().tags;
    Ok(KnowledgeOutput::Tags { tags })
}
