//! Launch-time runtime parsing and validation (`ur_config::WorkerRuntime`).
//!
//! Resolution itself lives in `WorkerModesConfig::resolve_runtime`; this module
//! turns the request's `runtime` string into an override and decides whether a
//! resolved runtime may launch. Every rejection here happens before any slot
//! acquisition, filesystem, or container work.

use ur_config::{AgentType, WorkerRuntime};

use crate::grpc::CoreError;

/// Parse `WorkerLaunchRequest.runtime`. Empty means "no override" (resolve from
/// config); an unknown value is a client error, never a silent fallback.
pub fn parse_requested_runtime(raw: &str) -> Result<Option<WorkerRuntime>, CoreError> {
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse()
        .map(Some)
        .map_err(|reason| CoreError::InvalidRuntime { reason })
}

/// Why `runtime = local` cannot be used for this launch, or `None` if it can.
///
/// Local workers are Claude-only and pool-backed: a `local = true` project has
/// no pool, and a `-w` workspace launch (or a launch with no project) has no
/// pool slot for the agent to run in.
pub fn local_runtime_rejection_reason(
    agent: AgentType,
    project_is_local: bool,
    has_pool_slot: bool,
) -> Option<String> {
    if !agent.supports_local_runtime() {
        return Some(format!(
            "agent '{}' does not support the local runtime (claude only)",
            agent.name()
        ));
    }
    if project_is_local {
        return Some(
            "local projects (local = true) cannot use the local runtime; they have no pool"
                .to_owned(),
        );
    }
    if !has_pool_slot {
        return Some("the local runtime requires a pool slot (-p <project> without -w)".to_owned());
    }
    None
}

/// Decide whether a launch with the resolved `runtime` may proceed.
/// Container launches always pass.
pub fn validate_launch_runtime(
    runtime: WorkerRuntime,
    agent: AgentType,
    project_is_local: bool,
    has_pool_slot: bool,
) -> Result<(), CoreError> {
    match runtime {
        WorkerRuntime::Container => Ok(()),
        WorkerRuntime::Local => {
            match local_runtime_rejection_reason(agent, project_is_local, has_pool_slot) {
                Some(reason) => Err(CoreError::LocalRuntimeUnsupported { reason }),
                None => Ok(()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tonic::{Code, Status};

    fn code_of(result: Result<(), CoreError>) -> (Code, String) {
        let status: Status = result.expect_err("should reject").into();
        (status.code(), status.message().to_owned())
    }

    #[test]
    fn parse_empty_is_no_override_and_unknown_is_invalid_argument() {
        assert_eq!(parse_requested_runtime("").unwrap(), None);
        assert_eq!(
            parse_requested_runtime("local").unwrap(),
            Some(WorkerRuntime::Local)
        );
        let status: Status = parse_requested_runtime("vm").unwrap_err().into();
        assert_eq!(status.code(), Code::InvalidArgument);
        assert!(status.message().contains("vm"), "{}", status.message());
    }

    #[test]
    fn container_always_passes() {
        for agent in AgentType::ALL {
            assert!(validate_launch_runtime(WorkerRuntime::Container, *agent, true, false).is_ok());
        }
    }

    #[test]
    fn local_rejects_unsupported_agents() {
        for agent in [AgentType::Codex, AgentType::Agy] {
            let (code, msg) = code_of(validate_launch_runtime(
                WorkerRuntime::Local,
                agent,
                false,
                true,
            ));
            assert_eq!(code, Code::FailedPrecondition);
            assert!(msg.contains(agent.name()), "{msg}");
        }
    }

    #[test]
    fn local_rejects_local_projects() {
        let (code, msg) = code_of(validate_launch_runtime(
            WorkerRuntime::Local,
            AgentType::Claude,
            true,
            false,
        ));
        assert_eq!(code, Code::FailedPrecondition);
        assert!(msg.contains("local = true"), "{msg}");
    }

    #[test]
    fn local_rejects_launches_without_a_pool_slot() {
        let (code, msg) = code_of(validate_launch_runtime(
            WorkerRuntime::Local,
            AgentType::Claude,
            false,
            false,
        ));
        assert_eq!(code, Code::FailedPrecondition);
        assert!(msg.contains("pool slot"), "{msg}");
    }

    #[test]
    fn valid_local_launch_passes() {
        assert!(
            validate_launch_runtime(WorkerRuntime::Local, AgentType::Claude, false, true).is_ok()
        );
    }
}
