/// CLI implementation of `ur ticket import <path>`.
///
/// Reads a JSONL file produced by `ur ticket export`, streams the records to
/// the server via the `TicketImport` client-streaming RPC, and reports the
/// number of rows inserted.
use std::io::{BufRead, BufReader};

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use ur_rpc::proto::ticket::TicketExportRecord;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

/// Run the import: read JSONL from `path` and stream records to the server.
///
/// Each line of the file must be a JSON object with at minimum a `"kind"` field
/// (identical to the format produced by `ur ticket export`).  The `kind` field
/// is extracted and used as the `TicketExportRecord::kind`, and the full line
/// (minus the `kind` field) is forwarded as `TicketExportRecord::json`.
///
/// Returns the number of rows inserted as reported by the server.
pub async fn execute_import<T>(path: &str, client: &mut TicketServiceClient<T>) -> Result<i64>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send + 'static,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let records = read_jsonl_records(path)?;

    // Build a channel-based stream to send records to the server.
    let (tx, rx) = mpsc::channel::<TicketExportRecord>(64);
    let stream = ReceiverStream::new(rx);

    // Spawn a task to send all records; the spawn allows the RPC call and the
    // sending to run concurrently (the channel acts as a bounded buffer).
    tokio::spawn(async move {
        for record in records {
            // If the receiver drops (RPC error), stop sending.
            if tx.send(record).await.is_err() {
                break;
            }
        }
    });

    let response = client
        .ticket_import(stream)
        .await
        .context("ticket_import RPC failed")?;

    Ok(response.into_inner().records_inserted)
}

/// Read all JSONL lines from `path` and convert them into `TicketExportRecord`
/// messages.
///
/// Each line is expected to be a JSON object containing a `"kind"` field.  The
/// `kind` field is stripped from the object and placed into the proto `kind`
/// field; the remaining object is placed into the proto `json` field.
fn read_jsonl_records(path: &str) -> Result<Vec<TicketExportRecord>> {
    let file =
        std::fs::File::open(path).with_context(|| format!("failed to open import file: {path}"))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();

    for (line_num, line) in reader.lines().enumerate() {
        let line = line.with_context(|| format!("failed to read line {}", line_num + 1))?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let (kind, json) = extract_kind(trimmed)
            .with_context(|| format!("invalid JSONL record on line {}", line_num + 1))?;

        records.push(TicketExportRecord { kind, json });
    }

    Ok(records)
}

/// Extract the `kind` field from a JSON object string and return `(kind, json_without_kind)`.
///
/// The `json_without_kind` is a valid JSON object with the `kind` key removed.
/// If the object only contained the `kind` key the result is `"{}"`.
fn extract_kind(json_obj: &str) -> Result<(String, String)> {
    let mut value: serde_json::Value =
        serde_json::from_str(json_obj).context("line is not valid JSON")?;

    let obj = value.as_object_mut().context("line is not a JSON object")?;

    let kind = obj
        .remove("kind")
        .context("missing 'kind' field in record")?;

    let kind_str = kind
        .as_str()
        .context("'kind' field is not a string")?
        .to_owned();

    if kind_str.is_empty() {
        bail!("'kind' field is empty");
    }

    let json = serde_json::to_string(&value).context("failed to re-serialize record")?;
    Ok((kind_str, json))
}

#[cfg(test)]
mod tests {
    use super::extract_kind;

    #[test]
    fn extract_kind_normal() {
        let line = r#"{"kind":"ticket","id":"ur-abc","title":"foo"}"#;
        let (kind, json) = extract_kind(line).unwrap();
        assert_eq!(kind, "ticket");
        // json must contain the id but not the kind
        assert!(json.contains("\"id\""));
        assert!(!json.contains("\"kind\""));
    }

    #[test]
    fn extract_kind_only_kind() {
        let line = r#"{"kind":"meta"}"#;
        let (kind, json) = extract_kind(line).unwrap();
        assert_eq!(kind, "meta");
        assert_eq!(json, "{}");
    }

    #[test]
    fn extract_kind_missing_kind_errors() {
        let line = r#"{"id":"ur-abc"}"#;
        assert!(extract_kind(line).is_err());
    }

    #[test]
    fn extract_kind_not_object_errors() {
        assert!(extract_kind("[]").is_err());
    }

    #[test]
    fn extract_kind_invalid_json_errors() {
        assert!(extract_kind("not-json").is_err());
    }
}
