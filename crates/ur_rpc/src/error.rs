use std::collections::HashMap;

use tonic::{Code, Status};
use tonic_types::{ErrorDetails, StatusExt};

// ---------------------------------------------------------------------------
// Generic reason codes
// ---------------------------------------------------------------------------
pub const NOT_FOUND: &str = "NOT_FOUND";
pub const INVALID_ARGUMENT: &str = "INVALID_ARGUMENT";
pub const INTERNAL: &str = "INTERNAL";
pub const UNAUTHENTICATED: &str = "UNAUTHENTICATED";
pub const PERMISSION_DENIED: &str = "PERMISSION_DENIED";
pub const UNAVAILABLE: &str = "UNAVAILABLE";

// ---------------------------------------------------------------------------
// Ticket reason codes
// ---------------------------------------------------------------------------
pub const TICKET_HAS_OPEN_CHILDREN: &str = "TICKET_HAS_OPEN_CHILDREN";

// ---------------------------------------------------------------------------
// HostExec reason codes
// ---------------------------------------------------------------------------
pub const COMMAND_NOT_ALLOWED: &str = "COMMAND_NOT_ALLOWED";
pub const TRANSFORM_REJECTED: &str = "TRANSFORM_REJECTED";
pub const BUILDERD_UNAVAILABLE: &str = "BUILDERD_UNAVAILABLE";

// ---------------------------------------------------------------------------
// RAG reason codes
// ---------------------------------------------------------------------------
pub const DOCS_NOT_INDEXED: &str = "DOCS_NOT_INDEXED";

// ---------------------------------------------------------------------------
// Domain constants
// ---------------------------------------------------------------------------
pub const DOMAIN_CORE: &str = "ur.core";
pub const DOMAIN_TICKET: &str = "ur.ticket";
pub const DOMAIN_HOSTEXEC: &str = "ur.hostexec";
pub const DOMAIN_RAG: &str = "ur.rag";
pub const DOMAIN_BUILDER: &str = "ur.builder";

// ---------------------------------------------------------------------------
// Server-side helper
// ---------------------------------------------------------------------------

/// Build a `tonic::Status` enriched with `ErrorInfo` (domain + reason + metadata).
///
/// This is the single server-side constructor for structured errors. Callers
/// supply the gRPC status code, a human-readable message, the domain constant,
/// the reason code, and an optional metadata map.
pub fn status_with_info(
    code: Code,
    message: impl Into<String>,
    domain: impl Into<String>,
    reason: impl Into<String>,
    metadata: HashMap<String, String>,
) -> Status {
    let details = ErrorDetails::with_error_info(reason, domain, metadata);
    Status::with_error_details(code, message, details)
}

// ---------------------------------------------------------------------------
// Client-side helper
// ---------------------------------------------------------------------------

/// Format a `tonic::Status` into a human-readable string.
///
/// When the status carries `ErrorInfo` (set via [`status_with_info`]), the
/// output includes domain, reason, and any metadata entries. Otherwise falls
/// back to `"{Code}: {message}"`.
pub fn format_status(status: &Status) -> String {
    if let Some(info) = status.get_details_error_info() {
        let mut parts = vec![format!("{}: {}", status.code(), status.message())];
        if !info.domain.is_empty() || !info.reason.is_empty() {
            parts.push(format!("[{}/{}]", info.domain, info.reason));
        }
        for (k, v) in &info.metadata {
            parts.push(format!("  {k}: {v}"));
        }
        parts.join("\n")
    } else {
        format!("{}: {}", status.code(), status.message())
    }
}

// ---------------------------------------------------------------------------
// StatusResultExt trait
// ---------------------------------------------------------------------------

/// Extension trait on `Result<T, tonic::Status>` that converts to
/// `anyhow::Result<T>` with a contextual prefix derived from [`format_status`].
pub trait StatusResultExt<T> {
    /// Map the `tonic::Status` error into an `anyhow::Error`, prefixed with
    /// `ctx` for call-site context (e.g. the RPC name).
    fn with_status_context(self, ctx: &str) -> anyhow::Result<T>;
}

impl<T> StatusResultExt<T> for Result<T, Status> {
    fn with_status_context(self, ctx: &str) -> anyhow::Result<T> {
        self.map_err(|status| {
            let formatted = format_status(&status);
            anyhow::anyhow!("{ctx}: {formatted}")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_with_info_roundtrip() {
        let mut meta = HashMap::new();
        meta.insert("ticket_id".into(), "ur-abc".into());

        let status = status_with_info(
            Code::FailedPrecondition,
            "ticket has open children",
            DOMAIN_TICKET,
            TICKET_HAS_OPEN_CHILDREN,
            meta,
        );

        assert_eq!(status.code(), Code::FailedPrecondition);
        assert_eq!(status.message(), "ticket has open children");

        let info = status
            .get_details_error_info()
            .expect("should have ErrorInfo");
        assert_eq!(info.domain, DOMAIN_TICKET);
        assert_eq!(info.reason, TICKET_HAS_OPEN_CHILDREN);
        assert_eq!(
            info.metadata.get("ticket_id").map(|s| s.as_str()),
            Some("ur-abc")
        );
    }

    #[test]
    fn format_status_with_error_info() {
        let status = status_with_info(
            Code::NotFound,
            "not found",
            DOMAIN_CORE,
            NOT_FOUND,
            HashMap::new(),
        );
        let formatted = format_status(&status);
        assert!(formatted.contains("not found"), "formatted: {formatted}");
        assert!(formatted.contains("ur.core"), "formatted: {formatted}");
        assert!(formatted.contains("NOT_FOUND"), "formatted: {formatted}");
    }

    #[test]
    fn format_status_fallback() {
        let status = Status::not_found("gone");
        let formatted = format_status(&status);
        assert!(formatted.contains("gone"), "formatted: {formatted}");
        // Code displays as its description string, not the variant name
        assert!(formatted.contains(": gone"), "formatted: {formatted}");
    }

    #[test]
    fn with_status_context_converts_to_anyhow() {
        let status = Status::internal("boom");
        let result: Result<(), Status> = Err(status);
        let err = result.with_status_context("my_rpc").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("my_rpc"), "msg: {msg}");
        assert!(msg.contains("boom"), "msg: {msg}");
    }
}
