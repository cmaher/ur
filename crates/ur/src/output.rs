use serde::Serialize;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone)]
pub struct OutputManager {
    format: OutputFormat,
}

impl OutputManager {
    pub fn from_args(output_arg: Option<&str>) -> Self {
        let format = if let Some(arg) = output_arg {
            match arg {
                "json" => OutputFormat::Json,
                _ => OutputFormat::Text,
            }
        } else if let Ok(env_val) = std::env::var("OUTPUT_FORMAT") {
            match env_val.as_str() {
                "json" => OutputFormat::Json,
                _ => OutputFormat::Text,
            }
        } else {
            OutputFormat::Text
        };
        Self { format }
    }

    pub fn is_json(&self) -> bool {
        self.format == OutputFormat::Json
    }

    pub fn print_success<T: Serialize>(&self, data: &T) {
        match self.format {
            OutputFormat::Json => {
                let envelope = serde_json::json!({
                    "ok": true,
                    "data": data,
                });
                println!("{}", serde_json::to_string(&envelope).unwrap());
            }
            OutputFormat::Text => {
                // No-op — caller handles text output
            }
        }
    }

    pub fn print_text(&self, msg: &str) {
        match self.format {
            OutputFormat::Json => {
                let envelope = serde_json::json!({
                    "ok": true,
                    "data": { "message": msg },
                });
                println!("{}", serde_json::to_string(&envelope).unwrap());
            }
            OutputFormat::Text => {
                println!("{msg}");
            }
        }
    }

    pub fn print_items<T: Serialize>(&self, items: &[T], text_fn: impl Fn(&[T]) -> String) {
        match self.format {
            OutputFormat::Json => {
                let envelope = serde_json::json!({
                    "ok": true,
                    "data": items,
                });
                println!("{}", serde_json::to_string(&envelope).unwrap());
            }
            OutputFormat::Text => {
                println!("{}", text_fn(items));
            }
        }
    }

    pub fn print_error(&self, err: &StructuredError) {
        match self.format {
            OutputFormat::Json => {
                let envelope = serde_json::json!({
                    "ok": false,
                    "error": err,
                });
                eprintln!("{}", serde_json::to_string(&envelope).unwrap());
            }
            OutputFormat::Text => {
                eprintln!("Error: {}", err.message);
            }
        }
    }
}

#[derive(Serialize)]
pub struct StructuredError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    ServerNotRunning,
    NotFound,
    AlreadyExists,
    InvalidInput,
    GrpcError,
    IoError,
    ConfigError,
    InteractiveNotSupported,
    InternalError,
}

impl ErrorCode {
    pub fn exit_code(self) -> i32 {
        match self {
            Self::InternalError => 1,
            Self::ServerNotRunning => 2,
            Self::NotFound => 3,
            Self::AlreadyExists => 4,
            Self::InvalidInput => 5,
            Self::GrpcError => 6,
            Self::IoError => 7,
            Self::ConfigError => 8,
            Self::InteractiveNotSupported => 9,
        }
    }
}

impl StructuredError {
    pub fn from_anyhow(err: &anyhow::Error) -> Self {
        // Check for tonic::Status downcast
        if let Some(status) = err.downcast_ref::<tonic::Status>() {
            let code = match status.code() {
                tonic::Code::NotFound => ErrorCode::NotFound,
                tonic::Code::AlreadyExists => ErrorCode::AlreadyExists,
                tonic::Code::InvalidArgument => ErrorCode::InvalidInput,
                tonic::Code::Unavailable => ErrorCode::ServerNotRunning,
                _ => ErrorCode::GrpcError,
            };
            return Self {
                code,
                message: status.message().to_string(),
            };
        }

        // Check for std::io::Error
        if err.downcast_ref::<std::io::Error>().is_some() {
            return Self {
                code: ErrorCode::IoError,
                message: format!("{err:#}"),
            };
        }

        // Pattern match on error message strings
        let msg = format!("{err:#}");
        let msg_lower = msg.to_lowercase();

        let code = if msg_lower.contains("server is not running") {
            ErrorCode::ServerNotRunning
        } else if msg_lower.contains("not found") {
            ErrorCode::NotFound
        } else if msg_lower.contains("already exists") {
            ErrorCode::AlreadyExists
        } else if msg_lower.contains("invalid") || msg_lower.contains("validation") {
            ErrorCode::InvalidInput
        } else if msg_lower.contains("failed to load config") || msg_lower.contains("ur.toml") {
            ErrorCode::ConfigError
        } else if msg_lower.contains("failed to connect") || msg_lower.contains("grpc") {
            ErrorCode::GrpcError
        } else {
            ErrorCode::InternalError
        };

        Self { code, message: msg }
    }

    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

// ── Response structs for JSON output ──

#[derive(Serialize)]
#[allow(dead_code)]
pub struct ServerStatus {
    pub status: String,
}

#[derive(Serialize)]
pub struct WorkerLaunched {
    pub worker_id: String,
    pub container_id: String,
}

#[derive(Serialize)]
pub struct WorkerStopped {
    pub worker_id: String,
}

#[derive(Serialize)]
pub struct CredentialsSaved {
    pub paths: Vec<String>,
}

#[derive(Serialize)]
pub struct ProjectAdded {
    pub key: String,
    pub repo: String,
}

#[derive(Serialize)]
pub struct ProjectRemoved {
    pub key: String,
}

#[derive(Serialize)]
pub struct ProjectInfo {
    pub key: String,
    pub repo: String,
    pub name: String,
    pub pool_limit: u32,
    pub slots_in_use: usize,
}

#[derive(Serialize)]
pub struct BackupCreated {
    pub path: String,
}

#[derive(Serialize)]
pub struct BackupEntry {
    pub name: String,
    pub timestamp: String,
    pub size_bytes: u64,
}

#[derive(Serialize)]
pub struct BackupList {
    pub directory: String,
    pub retain_count: u64,
    pub backups: Vec<BackupEntry>,
}

#[derive(Serialize)]
pub struct DomainList {
    pub domains: Vec<String>,
}

#[derive(Serialize)]
pub struct ContainerKilled {
    pub container_id: String,
}

#[derive(Serialize)]
pub struct WorkerDir {
    pub path: String,
}

/// Resolve output format before clap parsing (for error formatting).
pub fn resolve_output_format_early() -> OutputFormat {
    let args: Vec<String> = std::env::args().collect();
    // Look for --output json in args
    for pair in args.windows(2) {
        if pair[0] == "--output" && pair[1] == "json" {
            return OutputFormat::Json;
        }
    }
    if let Ok(val) = std::env::var("OUTPUT_FORMAT")
        && val == "json"
    {
        return OutputFormat::Json;
    }
    OutputFormat::Text
}

/// Handle clap parse errors with optional JSON formatting.
pub fn handle_clap_error(err: clap::Error, format: OutputFormat) -> ! {
    match format {
        OutputFormat::Json => {
            let structured = StructuredError::new(ErrorCode::InvalidInput, err.to_string());
            let envelope = serde_json::json!({
                "ok": false,
                "error": structured,
            });
            eprintln!("{}", serde_json::to_string(&envelope).unwrap());
            std::process::exit(structured.code.exit_code());
        }
        OutputFormat::Text => {
            err.exit();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_exit_codes() {
        assert_eq!(ErrorCode::InternalError.exit_code(), 1);
        assert_eq!(ErrorCode::ServerNotRunning.exit_code(), 2);
        assert_eq!(ErrorCode::NotFound.exit_code(), 3);
        assert_eq!(ErrorCode::AlreadyExists.exit_code(), 4);
        assert_eq!(ErrorCode::InvalidInput.exit_code(), 5);
        assert_eq!(ErrorCode::GrpcError.exit_code(), 6);
        assert_eq!(ErrorCode::IoError.exit_code(), 7);
        assert_eq!(ErrorCode::ConfigError.exit_code(), 8);
        assert_eq!(ErrorCode::InteractiveNotSupported.exit_code(), 9);
    }

    #[test]
    fn structured_error_from_anyhow_server_not_running() {
        let err = anyhow::anyhow!("server is not running — run 'ur server start' first");
        let se = StructuredError::from_anyhow(&err);
        assert!(matches!(se.code, ErrorCode::ServerNotRunning));
    }

    #[test]
    fn structured_error_from_anyhow_not_found() {
        let err = anyhow::anyhow!("project 'foo' not found in config");
        let se = StructuredError::from_anyhow(&err);
        assert!(matches!(se.code, ErrorCode::NotFound));
    }

    #[test]
    fn structured_error_from_anyhow_already_exists() {
        let err = anyhow::anyhow!("project 'foo' already exists");
        let se = StructuredError::from_anyhow(&err);
        assert!(matches!(se.code, ErrorCode::AlreadyExists));
    }

    #[test]
    fn output_manager_text_mode() {
        let om = OutputManager {
            format: OutputFormat::Text,
        };
        assert!(!om.is_json());
    }

    #[test]
    fn output_manager_json_mode() {
        let om = OutputManager {
            format: OutputFormat::Json,
        };
        assert!(om.is_json());
    }

    #[test]
    fn structured_error_serializes() {
        let err = StructuredError::new(ErrorCode::NotFound, "thing not found");
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains("\"not_found\""));
        assert!(json.contains("thing not found"));
    }
}
