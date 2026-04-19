/// CLI implementation of `ur ticket export <path>`.
///
/// Calls the `TicketExport` streaming RPC and writes each record as a line of
/// JSONL to the requested file path (or stdout when the path is `-`).
use std::io::Write;

use anyhow::{Context, Result};
use tonic::Streaming;
use ur_rpc::proto::ticket::TicketExportRecord;
use ur_rpc::proto::ticket::TicketExportRequest;
use ur_rpc::proto::ticket::ticket_service_client::TicketServiceClient;

/// Run the export: stream records from the server and write them to `path`.
///
/// When `path` is `"-"`, output is written to stdout.
pub async fn execute_export<T>(path: &str, client: &mut TicketServiceClient<T>) -> Result<()>
where
    T: tonic::client::GrpcService<tonic::body::Body> + Send,
    T::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    T::ResponseBody: http_body::Body<Data = bytes::Bytes> + Send + 'static,
    <T::ResponseBody as http_body::Body>::Error:
        Into<Box<dyn std::error::Error + Send + Sync>> + Send,
{
    let mut stream = client
        .ticket_export(TicketExportRequest {})
        .await
        .context("ticket_export RPC failed")?
        .into_inner();

    if path == "-" {
        write_export_stream(&mut stream, &mut std::io::stdout()).await
    } else {
        let mut file = std::fs::File::create(path)
            .with_context(|| format!("failed to create output file: {path}"))?;
        write_export_stream(&mut stream, &mut file).await
    }
}

async fn write_export_stream<W: Write>(
    stream: &mut Streaming<TicketExportRecord>,
    out: &mut W,
) -> Result<()> {
    while let Some(record) = stream.message().await.context("stream error from server")? {
        // Each record's json field is already a JSON object string.
        // Emit: {"kind":"<kind>",<rest of json object fields>}
        let json_obj = record.json.trim();
        // json_obj is a JSON object like {"id":"...","project":"...",...}
        // We need to inject "kind": "<kind>" as the first field.
        let line = inject_kind(&record.kind, json_obj)?;
        writeln!(out, "{line}").context("failed to write export record")?;
    }

    Ok(())
}

/// Inject `"_kind": "<kind>"` as the first field of a JSON object string.
///
/// Assumes `json_obj` is a valid JSON object starting with `{`.
/// Returns the modified JSON string with `_kind` prepended.
pub(crate) fn inject_kind(kind: &str, json_obj: &str) -> Result<String> {
    // Fast path: empty object
    if json_obj == "{}" {
        return Ok(format!("{{\"_kind\":\"{kind}\"}}"));
    }

    // Insert after the opening brace
    let after_brace = json_obj
        .strip_prefix('{')
        .context("export record json is not a JSON object")?;

    let kind_escaped = serde_json::to_string(kind).context("failed to serialize kind")?;
    // kind_escaped includes the surrounding quotes, e.g. "\"ticket\""
    Ok(format!("{{\"_kind\":{kind_escaped},{after_brace}"))
}

#[cfg(test)]
mod tests {
    use super::inject_kind;

    #[test]
    fn inject_kind_normal() {
        let json = r#"{"id":"ur-abc","title":"foo"}"#;
        let result = inject_kind("ticket", json).unwrap();
        assert_eq!(result, r#"{"_kind":"ticket","id":"ur-abc","title":"foo"}"#);
    }

    #[test]
    fn inject_kind_empty_object() {
        let result = inject_kind("meta", "{}").unwrap();
        assert_eq!(result, r#"{"_kind":"meta"}"#);
    }

    #[test]
    fn inject_kind_escapes_special_chars() {
        let result = inject_kind("ticket_comment", r#"{"x":1}"#).unwrap();
        assert_eq!(result, r#"{"_kind":"ticket_comment","x":1}"#);
    }

    #[test]
    fn export_then_extract_preserves_edge_kind() {
        let payload = r#"{"source_id":"a","target_id":"b","kind":"blocks"}"#;
        let line = super::inject_kind("edge", payload).unwrap();

        let (record_kind, payload_out) = crate::ticket::import::extract_kind(&line).unwrap();
        assert_eq!(record_kind, "edge");

        let v: serde_json::Value = serde_json::from_str(&payload_out).unwrap();
        assert_eq!(v["kind"], "blocks");
    }
}
