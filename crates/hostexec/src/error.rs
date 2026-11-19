use std::collections::HashMap;

use tonic::{Code, Status};

use ur_rpc::error::{
    self, BUILDERD_UNAVAILABLE, COMMAND_NOT_ALLOWED, DOMAIN_HOSTEXEC, INTERNAL, INVALID_ARGUMENT,
    NOT_FOUND, SCRIPT_NOT_ALLOWED, TRANSFORM_REJECTED,
};

#[derive(Debug, thiserror::Error)]
pub enum HostExecError {
    #[error("command not allowed: {command}")]
    CommandNotAllowed { command: String },

    #[error("transform rejected: {reason}")]
    TransformRejected { reason: String },

    #[error("builderd unavailable")]
    BuilderdUnavailable,

    #[error("builderd exec failed: {message}")]
    BuilderdExecFailed { message: String },

    #[error("invalid worker ID: {reason}")]
    InvalidWorkerId { reason: String },

    #[error("worker not found: {worker_id}")]
    WorkerNotFound { worker_id: String },

    #[error("invalid working directory {path}: {reason}")]
    InvalidWorkingDir { path: String, reason: String },

    #[error("missing start frame")]
    MissingStartFrame,

    #[error("script not allowed: {script} for project {project}")]
    ScriptNotAllowed { script: String, project: String },
}

impl From<HostExecError> for Status {
    fn from(err: HostExecError) -> Self {
        match &err {
            HostExecError::CommandNotAllowed { command } => {
                let mut meta = HashMap::new();
                meta.insert("command".into(), command.clone());
                error::status_with_info(
                    Code::PermissionDenied,
                    err.to_string(),
                    DOMAIN_HOSTEXEC,
                    COMMAND_NOT_ALLOWED,
                    meta,
                )
            }
            HostExecError::TransformRejected { .. } => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                TRANSFORM_REJECTED,
                HashMap::new(),
            ),
            HostExecError::BuilderdUnavailable => error::status_with_info(
                Code::Unavailable,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                BUILDERD_UNAVAILABLE,
                HashMap::new(),
            ),
            HostExecError::BuilderdExecFailed { .. } => error::status_with_info(
                Code::Internal,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                INTERNAL,
                HashMap::new(),
            ),
            HostExecError::InvalidWorkerId { .. } => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                INVALID_ARGUMENT,
                HashMap::new(),
            ),
            HostExecError::WorkerNotFound { worker_id } => {
                let mut meta = HashMap::new();
                meta.insert("worker_id".into(), worker_id.clone());
                error::status_with_info(
                    Code::NotFound,
                    err.to_string(),
                    DOMAIN_HOSTEXEC,
                    NOT_FOUND,
                    meta,
                )
            }
            HostExecError::InvalidWorkingDir { path, .. } => {
                let mut meta = HashMap::new();
                meta.insert("path".into(), path.clone());
                error::status_with_info(
                    Code::InvalidArgument,
                    err.to_string(),
                    DOMAIN_HOSTEXEC,
                    INVALID_ARGUMENT,
                    meta,
                )
            }
            HostExecError::MissingStartFrame => error::status_with_info(
                Code::InvalidArgument,
                err.to_string(),
                DOMAIN_HOSTEXEC,
                INVALID_ARGUMENT,
                HashMap::new(),
            ),
            HostExecError::ScriptNotAllowed { script, project } => {
                let mut meta = HashMap::new();
                meta.insert("script".into(), script.clone());
                meta.insert("project".into(), project.clone());
                error::status_with_info(
                    Code::PermissionDenied,
                    err.to_string(),
                    DOMAIN_HOSTEXEC,
                    SCRIPT_NOT_ALLOWED,
                    meta,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- ScriptNotAllowed error variant status mapping ---

    #[test]
    fn script_not_allowed_maps_to_permission_denied() {
        let err = HostExecError::ScriptNotAllowed {
            script: "scripts/deploy.sh".into(),
            project: "myproject".into(),
        };
        let status: Status = err.into();
        assert_eq!(status.code(), Code::PermissionDenied);
    }

    #[test]
    fn script_not_allowed_status_has_error_info() {
        use tonic_types::StatusExt;

        let err = HostExecError::ScriptNotAllowed {
            script: "scripts/deploy.sh".into(),
            project: "myproject".into(),
        };
        let status: Status = err.into();
        let info = status
            .get_details_error_info()
            .expect("should have ErrorInfo");
        assert_eq!(info.reason, SCRIPT_NOT_ALLOWED);
        assert_eq!(info.domain, DOMAIN_HOSTEXEC);
        assert_eq!(
            info.metadata.get("script").map(|s| s.as_str()),
            Some("scripts/deploy.sh")
        );
        assert_eq!(
            info.metadata.get("project").map(|s| s.as_str()),
            Some("myproject")
        );
    }
}
