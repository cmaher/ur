use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Where a worker's agent runs: in a Docker container (today's only launch
/// path) or natively on the host in a pool slot.
///
/// Selected by the `runtime` key in `ur.toml` (top level, `[projects.<key>]`,
/// `[worker_modes.<mode>]`) or `ur worker launch --runtime`. Serde handles the
/// `"container" | "local"` spelling, so an unknown value is a config error.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkerRuntime {
    #[default]
    Container,
    Local,
}

impl WorkerRuntime {
    pub const ALL: &'static [WorkerRuntime] = &[WorkerRuntime::Container, WorkerRuntime::Local];

    /// Stable lowercase name, as persisted in `workflow_db` and sent on the wire.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Local => "local",
        }
    }
}

impl fmt::Display for WorkerRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for WorkerRuntime {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .iter()
            .copied()
            .find(|runtime| runtime.as_str() == s)
            .ok_or_else(|| {
                let valid: Vec<&str> = Self::ALL.iter().map(Self::as_str).collect();
                format!("unknown worker runtime '{s}'. Valid runtimes: {valid:?}")
            })
    }
}

/// Resolve a worker's runtime, highest priority first: the launch request
/// (`--runtime`), the mode's `runtime`, the project's `runtime`, then the
/// top-level `runtime` (which itself defaults to container).
///
/// Workflow-dispatched workers have no CLI and pass `requested = None`.
pub fn resolve_worker_runtime(
    requested: Option<WorkerRuntime>,
    mode: Option<WorkerRuntime>,
    project: Option<WorkerRuntime>,
    top_level: WorkerRuntime,
) -> WorkerRuntime {
    requested.or(mode).or(project).unwrap_or(top_level)
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: WorkerRuntime = WorkerRuntime::Container;
    const L: WorkerRuntime = WorkerRuntime::Local;

    #[test]
    fn requested_wins_over_everything() {
        assert_eq!(resolve_worker_runtime(Some(C), Some(L), Some(L), L), C);
    }

    #[test]
    fn mode_wins_over_project_and_top_level() {
        assert_eq!(resolve_worker_runtime(None, Some(L), Some(C), C), L);
    }

    #[test]
    fn project_wins_over_top_level() {
        assert_eq!(resolve_worker_runtime(None, None, Some(L), C), L);
    }

    #[test]
    fn top_level_used_when_nothing_else_set() {
        assert_eq!(resolve_worker_runtime(None, None, None, L), L);
    }

    #[test]
    fn default_is_container() {
        assert_eq!(
            resolve_worker_runtime(None, None, None, WorkerRuntime::default()),
            C
        );
    }

    #[test]
    fn string_round_trip_and_rejects_unknown() {
        for runtime in WorkerRuntime::ALL {
            assert_eq!(runtime.as_str().parse::<WorkerRuntime>(), Ok(*runtime));
        }
        let err = "vm".parse::<WorkerRuntime>().unwrap_err();
        assert!(err.contains("unknown worker runtime 'vm'"), "{err}");
    }
}
