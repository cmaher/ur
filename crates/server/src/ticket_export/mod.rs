/// Server-side implementation of the `TicketExport` streaming RPC.
///
/// Reads all ticket-domain rows from `ticket_db` in a deterministic order and
/// yields one `TicketExportRecord` per row.  Each record carries a `kind`
/// discriminator (`ticket`, `edge`, `meta`, `activity`, `ticket_comment`) and a
/// `json` payload serialized with `serde_json`.
use serde::Serialize;
use ticket_db::TicketRepo;
use tonic::Status;
use ur_rpc::proto::ticket::TicketExportRecord;

/// Serialize one export row into a `TicketExportRecord`.
///
/// `kind` is the discriminator string; `payload` is any `Serialize`-able value.
#[allow(clippy::result_large_err)]
fn record<T: Serialize>(kind: &'static str, payload: &T) -> Result<TicketExportRecord, Status> {
    let json = serde_json::to_string(payload)
        .map_err(|e| Status::internal(format!("serialization failed: {e}")))?;
    Ok(TicketExportRecord {
        kind: kind.to_owned(),
        json,
    })
}

/// Stream all ticket-domain rows as `TicketExportRecord` messages.
///
/// Ordering within each kind is stable (by primary key / natural sort).
/// Kinds are streamed in this fixed sequence:
///   ticket → edge → meta → activity → ticket_comment
pub async fn stream_export(
    ticket_repo: &TicketRepo,
    sender: &tokio::sync::mpsc::Sender<Result<TicketExportRecord, Status>>,
) -> Result<(), Status> {
    stream_tickets(ticket_repo, sender).await?;
    stream_edges(ticket_repo, sender).await?;
    stream_meta(ticket_repo, sender).await?;
    stream_activities(ticket_repo, sender).await?;
    stream_ticket_comments(ticket_repo, sender).await
}

async fn stream_tickets(
    ticket_repo: &TicketRepo,
    sender: &tokio::sync::mpsc::Sender<Result<TicketExportRecord, Status>>,
) -> Result<(), Status> {
    let rows = ticket_repo
        .export_tickets()
        .await
        .map_err(|e| Status::internal(format!("failed to export tickets: {e}")))?;

    for row in rows {
        let rec = record(
            "ticket",
            &TicketRow {
                id: &row.id,
                project: &row.project,
                type_: &row.type_,
                status: &row.status,
                lifecycle_status: &row.lifecycle_status,
                lifecycle_managed: row.lifecycle_managed,
                priority: row.priority,
                parent_id: row.parent_id.as_deref(),
                title: &row.title,
                body: &row.body,
                branch: row.branch.as_deref(),
                created_at: &row.created_at,
                updated_at: &row.updated_at,
            },
        )?;
        sender
            .send(Ok(rec))
            .await
            .map_err(|_| Status::internal("export stream closed"))?;
    }

    Ok(())
}

async fn stream_edges(
    ticket_repo: &TicketRepo,
    sender: &tokio::sync::mpsc::Sender<Result<TicketExportRecord, Status>>,
) -> Result<(), Status> {
    let rows = ticket_repo
        .export_edges()
        .await
        .map_err(|e| Status::internal(format!("failed to export edges: {e}")))?;

    for row in rows {
        let rec = record(
            "edge",
            &EdgeRow {
                source_id: &row.source_id,
                target_id: &row.target_id,
                kind: &row.kind,
            },
        )?;
        sender
            .send(Ok(rec))
            .await
            .map_err(|_| Status::internal("export stream closed"))?;
    }

    Ok(())
}

async fn stream_meta(
    ticket_repo: &TicketRepo,
    sender: &tokio::sync::mpsc::Sender<Result<TicketExportRecord, Status>>,
) -> Result<(), Status> {
    let rows = ticket_repo
        .export_meta()
        .await
        .map_err(|e| Status::internal(format!("failed to export meta: {e}")))?;

    for row in rows {
        let rec = record(
            "meta",
            &MetaRow {
                entity_id: &row.entity_id,
                entity_type: &row.entity_type,
                key: &row.key,
                value: &row.value,
            },
        )?;
        sender
            .send(Ok(rec))
            .await
            .map_err(|_| Status::internal("export stream closed"))?;
    }

    Ok(())
}

async fn stream_activities(
    ticket_repo: &TicketRepo,
    sender: &tokio::sync::mpsc::Sender<Result<TicketExportRecord, Status>>,
) -> Result<(), Status> {
    let rows = ticket_repo
        .export_activities()
        .await
        .map_err(|e| Status::internal(format!("failed to export activities: {e}")))?;

    for row in rows {
        let rec = record(
            "activity",
            &ActivityRow {
                id: &row.id,
                ticket_id: &row.ticket_id,
                timestamp: &row.timestamp,
                author: &row.author,
                message: &row.message,
            },
        )?;
        sender
            .send(Ok(rec))
            .await
            .map_err(|_| Status::internal("export stream closed"))?;
    }

    Ok(())
}

async fn stream_ticket_comments(
    ticket_repo: &TicketRepo,
    sender: &tokio::sync::mpsc::Sender<Result<TicketExportRecord, Status>>,
) -> Result<(), Status> {
    let rows = ticket_repo
        .export_ticket_comments()
        .await
        .map_err(|e| Status::internal(format!("failed to export ticket_comments: {e}")))?;

    for row in rows {
        let rec = record(
            "ticket_comment",
            &TicketCommentRow {
                comment_id: &row.comment_id,
                ticket_id: &row.ticket_id,
                pr_number: row.pr_number,
                gh_repo: &row.gh_repo,
                reply_posted: row.reply_posted,
                created_at: &row.created_at,
            },
        )?;
        sender
            .send(Ok(rec))
            .await
            .map_err(|_| Status::internal("export stream closed"))?;
    }

    Ok(())
}

// --- Private serialization shapes ---

#[derive(Serialize)]
struct TicketRow<'a> {
    id: &'a str,
    project: &'a str,
    #[serde(rename = "type")]
    type_: &'a str,
    status: &'a str,
    lifecycle_status: &'a str,
    lifecycle_managed: bool,
    priority: i32,
    parent_id: Option<&'a str>,
    title: &'a str,
    body: &'a str,
    branch: Option<&'a str>,
    created_at: &'a str,
    updated_at: &'a str,
}

#[derive(Serialize)]
struct EdgeRow<'a> {
    source_id: &'a str,
    target_id: &'a str,
    kind: &'a str,
}

#[derive(Serialize)]
struct MetaRow<'a> {
    entity_id: &'a str,
    entity_type: &'a str,
    key: &'a str,
    value: &'a str,
}

#[derive(Serialize)]
struct ActivityRow<'a> {
    id: &'a str,
    ticket_id: &'a str,
    timestamp: &'a str,
    author: &'a str,
    message: &'a str,
}

#[derive(Serialize)]
struct TicketCommentRow<'a> {
    comment_id: &'a str,
    ticket_id: &'a str,
    pr_number: i64,
    gh_repo: &'a str,
    reply_posted: bool,
    created_at: &'a str,
}
