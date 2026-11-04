//! Lifecycle of host-local workers: a tmux session in the pool slot running
//! the agent, plus the worker's in-process `WorkerDaemonServiceImpl`.
//!
//! Launch order (ur-4p34c): validate id/env → build drive (validates agent)
//! → sync startup hooks
//! → tmux session → background hooks in window `:bg` → registry insert →
//! agent argv typed into window 0. Any failure after the drive build runs the
//! full teardown before the error is returned.
//!
//! Invariant (ur-j5puq): once [`LocalWorkerManager::adopt_existing`] has
//! run, a `ur-<id>` session on the socket whose `UR_DRIVE` is under this
//! builderd's drive root exists only while `<id>` is registered. Every ur
//! instance on the host shares the socket; sessions with a drive elsewhere
//! belong to another instance and are never adopted, killed or listed. Teardown kills the session first and deregisters last, so a
//! failed kill or drive removal leaves the worker listed for a retried Stop.
//!
//! # Known limits
//!
//! - Sync startup hooks run with builderd's environment, not the session env.
//! - An adopted design worker whose agent had already returned to the shell
//!   before the restart is not detected as exited (its watcher starts in the
//!   waiting state); a Stop still tears it down.
//! - Orphan drive listing/teardown failures at startup are logged only and
//!   retried at the next startup: drives hold only links, so failing startup
//!   over one would cost availability for no data-safety gain.

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

    /// Registered ids, sorted.
    fn ids(&self) -> Vec<String> {
        let mut out: Vec<_> = self.read().keys().cloned().collect();
        out.sort();
        out
    }

    fn abort_watcher(&self, worker_id: &str) {
        if let Some(w) = self.read().get(worker_id) {
            w.watcher.abort();
        }
    }

    fn insert(&self, worker_id: String, worker: LocalWorker) {
        self.write().insert(worker_id, worker);
    }

    fn remove(&self, worker_id: &str) {
        self.write().remove(worker_id);
    }

    /// Slots of registered workers.
    fn slots(&self) -> Vec<PathBuf> {
        self.read().values().map(|w| w.slot_path.clone()).collect()
    }

    /// What teardown needs, without removing the entry.
    fn teardown_target(&self, worker_id: &str) -> Option<(PathBuf, AgentType)> {
        self.read()
            .get(worker_id)
            .map(|w| (w.slot_path.clone(), w.agent))
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
    /// This builderd's address as seen from the host, exported as
    /// `UR_WORKERD_ADDR` so `ur-workertools` reaches the routed daemon.
    workerd_addr: String,
    /// Replaces the agent program (argv[0]); `None` in production.
    agent_program: Option<PathBuf>,
}

impl LocalWorkerManager {
    pub fn new(
        workspace: LocalWorkspaceManager,
        registry: LocalWorkerRegistry,
        tmux_socket: String,
        exit_poll_interval: Duration,
        workerd_addr: String,
        agent_program: Option<PathBuf>,
    ) -> Self {
        LocalWorkerManager {
            workspace,
            registry,
            tmux_socket,
            exit_poll_interval,
            workerd_addr,
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
        validate_launch(&req.worker_id, &env)?;
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

        let identity = WorkerIdentity {
            server_addr: env_value(&env, ur_config::UR_SERVER_ADDR_ENV).to_owned(),
            worker_id: req.worker_id.clone(),
            worker_secret: env_value(&env, ur_config::UR_WORKER_SECRET_ENV).to_owned(),
        };
        let session = tmux::Session::create(tmux::CreateOptions {
            name: session_name(&req.worker_id),
            socket: Some(self.tmux_socket.clone()),
            start_dir: Some(req.slot_path.clone()),
            env: session_env(req, drive, &self.workerd_addr, env),
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
                .new_window(BG_WINDOW, Some(&req.slot_path))
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
        let watcher = self.spawn_exit_watcher(
            &req.worker_id,
            req.instruction_strategy.as_deref() == Some(DESIGN_STRATEGY),
            &session,
        );
        self.registry.insert(
            req.worker_id.clone(),
            LocalWorker {
                daemon,
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
        worker_id: &str,
        is_design: bool,
        session: &tmux::Session,
    ) -> AbortHandle {
        let sink = LocalExitSink {
            manager: self.clone(),
            worker_id: worker_id.to_owned(),
        };
        let watcher = ExitWatcherManager::new(
            Arc::new(session.clone()),
            Arc::new(sink),
            is_design,
            self.exit_poll_interval,
        );
        let worker_id = worker_id.to_owned();
        // The server learns of the exit from ListLocalWorkers (its reconcile
        // loop); a failed teardown leaves the worker registered for Stop.
        tokio::spawn(async move {
            match watcher.run().await {
                Ok(reason) => info!(worker_id, ?reason, "local worker agent exited"),
                Err(e) => error!(worker_id, error = %format!("{e:#}"), "local worker exit teardown failed"),
            }
        })
        .abort_handle()
    }

    /// Stop a worker. Unknown or already-stopped ids succeed once their
    /// session is gone.
    ///
    /// An unregistered id still has its `ur-<id>` session killed (a failed
    /// launch whose own cleanup kill failed can leave one): Stop never
    /// succeeds while that session may be alive.
    pub async fn stop(&self, worker_id: &str) -> Result<()> {
        if self.registry.contains(worker_id) {
            return self.teardown_known(worker_id, TeardownCaller::Stop).await;
        }
        self.session(worker_id)
            .kill()
            .await
            .map_err(|e| Status::internal(format!("kill tmux session: {e:#}")))
    }

    async fn teardown_known(&self, worker_id: &str, caller: TeardownCaller) -> Result<()> {
        let Some((slot_path, agent)) = self.registry.teardown_target(worker_id) else {
            return Ok(());
        };
        self.teardown(worker_id, &slot_path, agent, caller).await
    }

    /// Kill the session, tear down the workspace, deregister. A failed step
    /// stops there and leaves the worker registered, so it stays listed and
    /// the server's retried Stop runs teardown again: an id is never dropped
    /// from the list while its session may be alive.
    async fn teardown(
        &self,
        worker_id: &str,
        slot_path: &Path,
        agent: AgentType,
        caller: TeardownCaller,
    ) -> Result<()> {
        self.session(worker_id)
            .kill()
            .await
            .map_err(|e| Status::internal(format!("kill tmux session: {e:#}")))?;
        if caller == TeardownCaller::Stop {
            self.registry.abort_watcher(worker_id);
        }
        self.workspace.teardown(worker_id, slot_path, agent).await?;
        self.registry.remove(worker_id);
        Ok(())
    }

    /// builderd startup (before serving): re-register every live `ur-<id>`
    /// session from its own env and start dir, kill any it cannot adopt, then
    /// tear down every drive left without a registered worker. Fails when the
    /// sessions cannot be listed or read, or an unadoptable one survives a
    /// kill, since serving then could under-report live sessions. Orphan
    /// drive failures are only logged (drives hold links only; the next
    /// startup retries).
    pub async fn adopt_existing(&self) -> Result<()> {
        let sessions = match tmux::Session::list_sessions(Some(&self.tmux_socket)).await {
            Ok(sessions) => sessions,
            Err(e) if tmux_missing(&e) => Vec::new(),
            Err(e) => {
                return Err(Status::internal(format!(
                    "list local worker sessions: {e:#}"
                )));
            }
        };
        for name in sessions {
            let Some(worker_id) = name.strip_prefix("ur-") else {
                continue;
            };
            self.adopt_or_kill(worker_id).await?;
        }
        let drives = self.workspace.drive_ids().await.unwrap_or_else(|e| {
            error!(error = e.message(), "cannot list local worker drives");
            Vec::new()
        });
        let live_slots = self.registry.slots();
        for worker_id in drives {
            if self.registry.contains(&worker_id) {
                continue;
            }
            info!(worker_id, "tearing down orphan local worker drive");
            if let Err(e) = self
                .workspace
                .teardown_orphan(&worker_id, &live_slots)
                .await
            {
                error!(
                    worker_id,
                    error = e.message(),
                    "orphan local worker drive teardown failed"
                );
            }
        }
        Ok(())
    }

    /// Adopt `ur-<worker_id>`, or kill it when its env is unusable. A tmux
    /// error reading a live session is fatal, not a reason to kill it. A
    /// session whose `UR_DRIVE` is outside this builderd's drive root is
    /// another ur instance's and is left alone (a missing `UR_DRIVE` counts
    /// as ours and unusable).
    async fn adopt_or_kill(&self, worker_id: &str) -> Result<()> {
        let session = self.session(worker_id);
        let read = async {
            Ok::<_, anyhow::Error>((session.environment().await?, session.start_dir().await?))
        }
        .await;
        let (env, slot_path) = match read {
            Ok(read) => read,
            Err(e) => {
                let alive = session.exists().await.map_err(|x| {
                    Status::internal(format!("check session '{}': {x:#}", session.name()))
                })?;
                return session_read_failure(worker_id, &e, alive);
            }
        };
        let drive = env_value(&env, UR_DRIVE_ENV);
        if !drive.is_empty() && !Path::new(drive).starts_with(self.workspace.drive_root()) {
            info!(
                worker_id,
                drive, "session belongs to another ur instance; leaving it"
            );
            return Ok(());
        }
        let agent = match adoptable(worker_id, &env, &self.workspace.drive_path(worker_id)) {
            Ok(agent) => agent,
            Err(reason) => {
                error!(
                    worker_id,
                    reason, "cannot adopt local worker session; killing it"
                );
                return session.kill().await.map_err(|k| {
                    Status::internal(format!(
                        "kill unadoptable session '{}': {k:#}",
                        session.name()
                    ))
                });
            }
        };
        let identity = WorkerIdentity {
            server_addr: env_value(&env, ur_config::UR_SERVER_ADDR_ENV).to_owned(),
            worker_id: worker_id.to_owned(),
            worker_secret: env_value(&env, ur_config::UR_WORKER_SECRET_ENV).to_owned(),
        };
        let is_design =
            env_value(&env, ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV) == DESIGN_STRATEGY;
        let watcher = self.spawn_exit_watcher(worker_id, is_design, &session);
        let daemon =
            WorkerDaemonServiceImpl::new(identity, agent, SkillLayout::ur_plugin(), session);
        self.registry.insert(
            worker_id.to_owned(),
            LocalWorker {
                daemon,
                slot_path,
                agent,
                watcher,
            },
        );
        info!(worker_id, "adopted live local worker session");
        Ok(())
    }

    /// Registered workers (sorted by id) and whether each session is alive.
    pub async fn list(&self) -> Result<Vec<(String, bool)>> {
        let mut out = Vec::new();
        for id in self.registry.ids() {
            let alive =
                self.session(&id).exists().await.map_err(|e| {
                    Status::internal(format!("check tmux session for '{id}': {e:#}"))
                })?;
            out.push((id, alive));
        }
        Ok(out)
    }
}

/// A failed read of a listed session: fatal while it is still alive (a
/// transient tmux error must not kill a live agent); nothing to adopt once it
/// has ended (its drive is left to the orphan sweep).
fn session_read_failure(worker_id: &str, e: &anyhow::Error, alive: bool) -> Result<()> {
    if alive {
        return Err(Status::internal(format!(
            "read local worker session '{}': {e:#}",
            session_name(worker_id)
        )));
    }
    info!(worker_id, "local worker session ended before adoption");
    Ok(())
}

/// Whether a session's env can be adopted as `worker_id`; the agent, or why
/// not. Only bad or missing env and an unsupported agent are unadoptable.
fn adoptable(
    worker_id: &str,
    env: &[(String, String)],
    drive: &Path,
) -> std::result::Result<AgentType, String> {
    if env_value(env, ur_config::UR_WORKER_ID_ENV) != worker_id {
        return Err(format!(
            "{} does not match the session name",
            ur_config::UR_WORKER_ID_ENV
        ));
    }
    validate_launch(worker_id, env).map_err(|s| s.message().to_owned())?;
    let agent = AgentType::parse(env_value(env, ur_config::UR_AGENT_TYPE_ENV))
        .map_err(|e| e.to_string())?;
    if !agent.supports_local_runtime() {
        return Err(format!(
            "agent '{}' does not support the local runtime",
            agent.name()
        ));
    }
    if Path::new(env_value(env, UR_DRIVE_ENV)) != drive {
        return Err(format!("{UR_DRIVE_ENV} is not this builderd's drive"));
    }
    Ok(agent)
}

/// Whether tmux is not installed: then no session can exist.
fn tmux_missing(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.downcast_ref::<std::io::Error>()
            .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
    })
}

/// Tears the worker down when its agent exits; the server sees it leave
/// `ListLocalWorkers`.
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

/// Server-supplied env every local worker needs for its server-bound calls.
const REQUIRED_ENV: &[&str] = &[
    ur_config::UR_SERVER_ADDR_ENV,
    ur_config::UR_WORKER_SECRET_ENV,
];

/// Reject ids tmux would rename (`.`/`:` → `_`, so kill/has-session miss the
/// session) and a request env missing the server variables.
fn validate_launch(worker_id: &str, env: &[(String, String)]) -> Result<()> {
    if worker_id.contains(['.', ':']) {
        return Err(Status::invalid_argument(format!(
            "worker id '{worker_id}' must not contain '.' or ':'"
        )));
    }
    for key in REQUIRED_ENV {
        if env_value(env, key).is_empty() {
            return Err(Status::invalid_argument(format!(
                "request env must set {key}"
            )));
        }
    }
    Ok(())
}

fn env_value<'a>(env: &'a [(String, String)], key: &str) -> &'a str {
    env.iter()
        .rev()
        .find(|(k, _)| k == key)
        .map_or("", |(_, v)| v.as_str())
}

/// Request env, minus anything builderd sets, followed by builderd's own.
fn session_env(
    req: &LocalWorkspaceRequest,
    drive: &Path,
    workerd_addr: &str,
    env: Vec<(String, String)>,
) -> Vec<(String, String)> {
    // Adoption after a builderd restart reads these back.
    let mut own = vec![
        (ur_config::UR_WORKER_ID_ENV, req.worker_id.clone()),
        (ur_config::UR_WORKERD_ADDR_ENV, workerd_addr.to_owned()),
        (ur_config::UR_AGENT_TYPE_ENV, req.agent.name().to_owned()),
        (UR_DRIVE_ENV, drive.to_string_lossy().into_owned()),
        (UR_RUNTIME_ENV, WorkerRuntime::Local.as_str().to_owned()),
    ];
    if let Some(strategy) = &req.instruction_strategy {
        own.push((
            ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV,
            strategy.clone(),
        ));
    }
    let mut out: Vec<(String, String)> = env
        .into_iter()
        .filter(|(k, _)| {
            k != ur_config::UR_WORKER_INSTRUCTION_STRATEGY_ENV
                && !own.iter().any(|(own_key, _)| own_key == k)
        })
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
