/// Server-side implementation of the `TicketImport` client-streaming RPC.
///
/// Buffers all incoming `TicketExportRecord` messages, checks for ID
/// collisions, then inserts everything in a single transaction:
///   tickets → edges → meta → activities → ticket_comments
use serde::Deserialize;
use ticket_db::{
    ExportActivity, ExportEdge, ExportMeta, ExportTicket, ExportTicketComment, ImportError,
    TicketRepo,
};
use tonic::Status;
use ur_rpc::proto::ticket::TicketExportRecord;

/// Deserialize shapes matching the export serialization in `ticket_export/mod.rs`.

#[derive(Deserialize)]
struct TicketJson {
    id: String,
    project: String,
    #[serde(rename = "type")]
    type_: String,
    status: String,
    lifecycle_status: String,
    lifecycle_managed: bool,
    priority: i32,
    parent_id: Option<String>,
    title: String,
    body: String,
    branch: Option<String>,
    created_at: String,
    updated_at: String,
}

#[derive(Deserialize)]
struct EdgeJson {
    source_id: String,
    target_id: String,
    kind: String,
}

#[derive(Deserialize)]
struct MetaJson {
    entity_id: String,
    entity_type: String,
    key: String,
    value: String,
}

#[derive(Deserialize)]
struct ActivityJson {
    id: String,
    ticket_id: String,
    timestamp: String,
    author: String,
    message: String,
}

#[derive(Deserialize)]
struct TicketCommentJson {
    comment_id: String,
    ticket_id: String,
    pr_number: i64,
    gh_repo: String,
    reply_posted: bool,
    created_at: String,
}

/// Process all buffered `TicketExportRecord` messages and import them into the
/// `ticket_db`.
///
/// Records are sorted into buckets by `kind`, then inserted via a single
/// transaction through `TicketRepo::import_records`.  Any unknown `kind`
/// values are silently skipped so the importer stays forward-compatible.
///
/// Returns the total number of rows inserted on success.
#[allow(clippy::result_large_err)]
pub async fn run_import(
    ticket_repo: &TicketRepo,
    records: Vec<TicketExportRecord>,
) -> Result<i64, Status> {
    let mut tickets: Vec<ExportTicket> = Vec::new();
    let mut edges: Vec<ExportEdge> = Vec::new();
    let mut meta: Vec<ExportMeta> = Vec::new();
    let mut activities: Vec<ExportActivity> = Vec::new();
    let mut comments: Vec<ExportTicketComment> = Vec::new();

    for record in records {
        match record.kind.as_str() {
            "ticket" => {
                let t: TicketJson = serde_json::from_str(&record.json)
                    .map_err(|e| Status::invalid_argument(format!("invalid ticket json: {e}")))?;
                tickets.push(ExportTicket {
                    id: t.id,
                    project: t.project,
                    type_: t.type_,
                    status: t.status,
                    lifecycle_status: t.lifecycle_status,
                    lifecycle_managed: t.lifecycle_managed,
                    priority: t.priority,
                    parent_id: t.parent_id,
                    title: t.title,
                    body: t.body,
                    branch: t.branch,
                    created_at: t.created_at,
                    updated_at: t.updated_at,
                });
            }
            "edge" => {
                let e: EdgeJson = serde_json::from_str(&record.json)
                    .map_err(|e| Status::invalid_argument(format!("invalid edge json: {e}")))?;
                edges.push(ExportEdge {
                    source_id: e.source_id,
                    target_id: e.target_id,
                    kind: e.kind,
                });
            }
            "meta" => {
                let m: MetaJson = serde_json::from_str(&record.json)
                    .map_err(|e| Status::invalid_argument(format!("invalid meta json: {e}")))?;
                meta.push(ExportMeta {
                    entity_id: m.entity_id,
                    entity_type: m.entity_type,
                    key: m.key,
                    value: m.value,
                });
            }
            "activity" => {
                let a: ActivityJson = serde_json::from_str(&record.json)
                    .map_err(|e| Status::invalid_argument(format!("invalid activity json: {e}")))?;
                activities.push(ExportActivity {
                    id: a.id,
                    ticket_id: a.ticket_id,
                    timestamp: a.timestamp,
                    author: a.author,
                    message: a.message,
                });
            }
            "ticket_comment" => {
                let c: TicketCommentJson = serde_json::from_str(&record.json).map_err(|e| {
                    Status::invalid_argument(format!("invalid ticket_comment json: {e}"))
                })?;
                comments.push(ExportTicketComment {
                    comment_id: c.comment_id,
                    ticket_id: c.ticket_id,
                    pr_number: c.pr_number,
                    gh_repo: c.gh_repo,
                    reply_posted: c.reply_posted,
                    created_at: c.created_at,
                });
            }
            // Unknown kinds are skipped for forward-compatibility.
            _ => {}
        }
    }

    ticket_repo
        .import_records(tickets, edges, meta, activities, comments)
        .await
        .map_err(|e| match e {
            ImportError::IdCollision(ids) => Status::already_exists(format!(
                "import aborted: ticket id collision(s): {}",
                ids.join(", ")
            )),
            ImportError::Db(msg) => Status::internal(format!("import database error: {msg}")),
        })
}
