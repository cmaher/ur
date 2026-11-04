//! Lifecycle of host-local workers: a tmux session in the pool slot running
//! the agent, plus the worker's in-process `WorkerDaemonServiceImpl`.
//!
//! Launch order (ur-4p34c): validate agent → build drive → sync startup hooks
//! → tmux session → background hooks in window `:bg` → registry insert →
//! agent argv typed into window 0. Any failure after the drive build runs the
//! full teardown before the error is returned.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use tokio::task::AbortHandle;
use tonic::Status;
use tracing::{error, info};
use ur_config::{AgentType, WorkerRuntime};
use workerd::{
    AgentExitReason, AgentExitSink, ExitWatcherManager, HookKind, SkillLayout, StartupHooksManager,
    WorkerDaemonServiceImpl, WorkerIdentity,
};

use crate::local_workspace::{LocalWorkspaceManager, LocalWorkspaceRequest};

/// Path of the worker's drive (`<workspace>/drive/<id>`).
pub const UR_DRIVE_ENV: &str = "UR_DRIVE";
/// Which runtime the worker runs under (`local` here).
pub const UR_RUNTIME_ENV: &str = "UR_RUNTIME";
/// Window that runs background startup hooks.
const BG_WINDOW: &str = "bg";
/// Strategy whose agent exit is detected by the pane returning to a shell.
const DESIGN_STRATEGY: &str = "design";

type Result<T> = std::result::Result<T, Status>;

/// tmux session name for a local worker.
pub fn session_name(worker_id: &str) -> String {
    format!("ur-{worker_id}")
}

/// One running local worker.
struct LocalWorker {
    daemon: WorkerDaemonServiceImpl,
    session: tmux::Session,
    slot_path: PathBuf,
    agent: AgentType,
    watcher: AbortHandle,
}

/// Live local workers by worker id.
#[derive(Clone, Default)]
pub struct LocalWorkerRegistry {
    inner: Arc<RwLock<HashMap<String, LocalWorker>>>,
}

impl LocalWorkerRegistry {
    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<String, LocalWorker>> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<String, LocalWorker>> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    /// The daemon serving `worker_id`, if it is registered.
    pub fn daemon(&self, worker_id: &str) -> Option<WorkerDaemonServiceImpl> {
        self.read().get(worker_id).map(|w| w.daemon.clone())
    }

    pub fn contains(&self, worker_id: &str) -> bool {
        self.read().contains_key(worker_id)
    }

    /// Registered ids (sorted) with their sessions.
    fn sessions(&self) -> Vec<(String, tmux::Session)> {
        let mut out: Vec<_> = self
            .read()
            .iter()
            .map(|(id, w)| (id.clone(), w.session.clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    fn insert(&self, worker_id: String, worker: LocalWorker) {
        self.write().insert(worker_id, worker);
    }

    fn remove(&self, worker_id: &str) -> Option<LocalWorker> {
        self.write().remove(worker_id)
    }

    /// What teardown needs, without removing the entry.
    fn teardown_target(&self, worker_id: &str) -> Option<(tmux::Session, PathBuf, AgentType)> {
        self.read()
            .get(worker_id)
            .map(|w| (w.session.clone(), w.slot_path.clone(), w.agent))
    }
}

/// Who called teardown; the exit watcher must not abort its own task.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TeardownCaller {
    Stop,
    ExitWatcher,
}

/// Launches, stops and lists local workers.
#[derive(Clone)]
pub struct LocalWorkerManager {
    workspace: LocalWorkspaceManager,
    registry: LocalWorkerRegistry,
    /// tmux server socket (`-L`); `ur_config::LOCAL_TMUX_SOCKET` in production.
    tmux_socket: String,
    exit_poll_interval: Duration,
    /// Replaces the agent program (argv[0]); `None` in production.
    agent_program: Option<PathBuf>,
}

impl LocalWorkerManager {
    pub fn new(
        workspace: LocalWorkspaceManager,
        registry: LocalWorkerRegistry,
        tmux_socket: String,
        exit_poll_interval: Duration,
        agent_program: Option<PathBuf>,
    ) -> Self {
        LocalWorkerManager {
            workspace,
            registry,
            tmux_socket,
            exit_poll_interval,
            agent_program,
        }
    }

    fn session(&self, worker_id: &str) -> tmux::Session {
        tmux::Session::on_socket(&self.tmux_socket, &session_name(worker_id))
    }

    /// Launch a worker. `env` is exported into the session after builderd's
    /// own variables, which it cannot override. Returns the drive path.
    pub async fn launch(
        &self,
        req: &LocalWorkspaceRequest,
        env: Vec<(String, String)>,
    ) -> Result<PathBuf> {
        if !req.agent.supports_local_runtime() {
            return Err(Status::failed_precondition(format!(
                "agent '{}' does not support the local runtime",
                req.agent.name()
            )));
        }
        let session = self.session(&req.worker_id);
        let orphan = session
            .exists()
            .await
            .map_err(|e| Status::internal(format!("check tmux session: {e:#}")))?;
        if self.registry.contains(&req.worker_id) || orphan {
            return Err(Status::already_exists(format!(
                "local worker '{}' is already running",
                req.worker_id
            )));
        }

        let built = self.workspace.build(req).await?;
        match self.start(req, &built.drive, built.argv, env).await {
            Ok(()) => Ok(built.drive),
            Err(err) => {
                let cleanup = self
                    .teardown(
                        &req.worker_id,
                        &session,
                        &req.slot_path,
                        req.agent,
                        TeardownCaller::Stop,
                    )
                    .await;
                Err(match cleanup {
                    Ok(()) => err,
                    Err(e) => Status::new(
                        err.code(),
                        format!("{}; teardown also failed: {}", err.message(), e.message()),
                    ),
                })
            }
        }
    }

    /// Everything after the drive build.
    async fn start(
        &self,
        req: &LocalWorkspaceRequest,
        drive: &Path,
        mut argv: Vec<String>,
        env: Vec<(String, String)>,
    ) -> Result<()> {
        let step =
            |name: &'static str| move |e: anyhow::Error| Status::internal(format!("{name}: {e:#}"));
        let hooks = StartupHooksManager::new(self.workspace.worker_paths(drive, req));
        hooks
            .run_sync()
            .await
            .map_err(|e| Status::failed_precondition(format!("sync startup hooks: {e:#}")))?;
        let bg_hooks = hooks
            .resolve(HookKind::Background)
            .map_err(step("resolve background startup hooks"))?;

        let identity = identity(&req.worker_id, &env);
        let session = tmux::Session::create(tmux::CreateOptions {
            name: session_name(&req.worker_id),
            socket: Some(self.tmux_socket.clone()),
            start_dir: Some(req.slot_path.clone()),
            env: session_env(req, drive, env),
            width: Some(220),
            height: Some(55),
            detached: true,
        })
        .await
        .map_err(step("create tmux session"))?;

        if !bg_hooks.is_empty() {
            let cmd = bg_hooks
                .iter()
                .map(|h| format!("{} &", shell_quote(&h.to_string_lossy())))
                .collect::<Vec<_>>()
                .join(" ")
                + " wait";
            session
                .new_window(BG_WINDOW)
                .await
                .map_err(step("create background hook window"))?;
            session
                .send_keys_to_window(BG_WINDOW, &cmd)
                .await
                .map_err(step("start background startup hooks"))?;
        }

        // Register before the agent starts: its SessionStart hook calls
        // NotifyIdle right away.
        let daemon = WorkerDaemonServiceImpl::new(
            identity,
            req.agent,
            SkillLayout::ur_plugin(),
            session.clone(),
        );
        let watcher = self.spawn_exit_watcher(req, &session);
        self.registry.insert(
            req.worker_id.clone(),
            LocalWorker {
                daemon,
                session: session.clone(),
                slot_path: req.slot_path.clone(),
                agent: req.agent,
                watcher,
            },
        );

        if let Some(program) = &self.agent_program {
            argv[0] = program.to_string_lossy().into_owned();
        }
        let cmd = argv
            .iter()
            .map(|a| shell_quote(a))
            .collect::<Vec<_>>()
            .join(" ");
        session.send_keys(&cmd).await.map_err(step("start agent"))?;
        info!(worker_id = req.worker_id, cmd, "local worker launched");
        Ok(())
    }

    fn spawn_exit_watcher(
        &self,
        req: &LocalWorkspaceRequest,
        session: &tmux::Session,
    ) -> AbortHandle {
        let sink = LocalExitSink {
            manager: self.clone(),
            worker_id: req.worker_id.clone(),
        };
        let watcher = ExitWatcherManager::new(
            Arc::new(session.clone()),
            Arc::new(sink),
            req.instruction_strategy.as_deref() == Some(DESIGN_STRATEGY),
            self.exit_poll_interval,
        );
        let worker_id = req.worker_id.clone();
        // Delivering this to the server is ur-j5puq; until then the sink's
        // error ends here.
        tokio::spawn(async move {
            match watcher.run().await {
                Ok(reason) => info!(worker_id, ?reason, "local worker agent exited"),
                Err(e) => error!(worker_id, error = %format!("{e:#}"), "local worker exit teardown failed"),
            }
        })
        .abort_handle()
    }

    /// Stop a worker. Unknown or already-stopped ids succeed.
    pub async fn stop(&self, worker_id: &str) -> Result<()> {
        self.teardown_known(worker_id, TeardownCaller::Stop).await
    }

    async fn teardown_known(&self, worker_id: &str, caller: TeardownCaller) -> Result<()> {
        let Some((session, slot_path, agent)) = self.registry.teardown_target(worker_id) else {
            return Ok(());
        };
        self.teardown(worker_id, &session, &slot_path, agent, caller)
            .await
    }

    /// Kill the session, deregister, tear down the workspace. Every step is
    /// attempted; the first error is returned.
    async fn teardown(
        &self,
        worker_id: &str,
        session: &tmux::Session,
        slot_path: &Path,
        agent: AgentType,
        caller: TeardownCaller,
    ) -> Result<()> {
        let mut first_err = session
            .kill()
            .await
            .err()
            .map(|e| Status::internal(format!("kill tmux session: {e:#}")));
        if let Some(worker) = self.registry.remove(worker_id)
            && caller == TeardownCaller::Stop
        {
            worker.watcher.abort();
        }
        if let Err(e) = self.workspace.teardown(worker_id, slot_path, agent).await {
            first_err.get_or_insert(e);
        }
        first_err.map_or(Ok(()), Err)
    }

    /// Registered workers (sorted by id) and whether each session is alive.
    pub async fn list(&self) -> Result<Vec<(String, bool)>> {
        let mut out = Vec::new();
        for (id, session) in self.registry.sessions() {
            let alive = session
                .exists()
                .await
                .map_err(|e| Status::internal(format!("check tmux session for '{id}': {e:#}")))?;
            out.push((id, alive));
        }
        Ok(out)
    }
}

/// Tears the worker down when its agent exits. Server notification is ur-j5puq.
struct LocalExitSink {
    manager: LocalWorkerManager,
    worker_id: String,
}

#[tonic::async_trait]
impl AgentExitSink for LocalExitSink {
    async fn agent_exited(&self, reason: AgentExitReason) -> anyhow::Result<()> {
        info!(
            worker_id = self.worker_id,
            ?reason,
            "tearing down exited local worker"
        );
        self.manager
            .teardown_known(&self.worker_id, TeardownCaller::ExitWatcher)
            .await
            .map_err(|s| anyhow::anyhow!("teardown of '{}': {}", self.worker_id, s.message()))
    }
}

fn env_value<'a>(env: &'a [(String, String)], key: &str) -> &'a str {
    env.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map_or("", |(_, v)| v.as_str())
}

/// The daemon's server identity, from the server-supplied env.
fn identity(worker_id: &str, env: &[(String, String)]) -> WorkerIdentity {
    WorkerIdentity {
        server_addr: env_value(env, ur_config::UR_SERVER_ADDR_ENV).to_owned(),
        worker_id: worker_id.to_owned(),
        worker_secret: env_value(env, ur_config::UR_WORKER_SECRET_ENV).to_owned(),
    }
}

/// Request env, minus anything builderd sets, followed by builderd's own.
fn session_env(
    req: &LocalWorkspaceRequest,
    drive: &Path,
    env: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let own = [
        (ur_config::UR_WORKER_ID_ENV, req.worker_id.clone()),
        (ur_config::UR_AGENT_TYPE_ENV, req.agent.name().to_owned()),
        (UR_DRIVE_ENV, drive.to_string_lossy().into_owned()),
        (UR_RUNTIME_ENV, WorkerRuntime::Local.as_str().to_owned()),
    ];
    let mut out: Vec<(String, String)> = env
        .into_iter()
        .filter(|(k, _)| !own.iter().any(|(own_key, _)| own_key == k))
        .collect();
    out.extend(own.into_iter().map(|(k, v)| (k.to_owned(), v)));
    out
}

/// POSIX single-quote `s` for typing into a shell.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(test)]
#[path = "local_worker_tests.rs"]
mod tests;
