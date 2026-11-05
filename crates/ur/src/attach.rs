//! `ur worker attach` / `launch -a` / `--rm`: attach the terminal to a
//! worker's agent tmux session, branching on the worker's runtime.
//!
//! - container: `docker exec -it <prefix><id> tmux -u attach-session -t =agent`
//! - local: `tmux -L <socket> -u attach-session -t =ur-<worker_id_full>` on the host

use std::future::Future;
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use container::{ContainerId, ContainerRuntime};
use tracing::info;
use ur_rpc::proto::core::core_service_client::CoreServiceClient;
use ur_rpc::proto::core::{WorkerListRequest, WorkerSummary};

/// The workerd-managed session inside every worker container.
const CONTAINER_SESSION: &str = "agent";

/// What attaching to one worker means for its runtime.
#[derive(Debug, Clone)]
pub enum AttachPlan {
    Container {
        container: ContainerId,
        /// Run inside the container via the runtime's interactive exec.
        command: Vec<String>,
    },
    Local {
        /// The CLI-facing id (process id), for error messages.
        worker_id: String,
        session: tmux::Session,
    },
}

#[derive(Clone)]
pub struct AttachManager {
    port: u16,
    worker_prefix: String,
    /// tmux server socket of local workers (`ur_config::LOCAL_TMUX_SOCKET`).
    tmux_socket: String,
    /// Host program for the interactive local attach exec only (`tmux` in
    /// production). The pre-attach `session.exists()` check always runs
    /// `tmux` from PATH.
    tmux_program: PathBuf,
}

impl AttachManager {
    pub fn new(
        port: u16,
        worker_prefix: String,
        tmux_socket: String,
        tmux_program: PathBuf,
    ) -> Self {
        Self {
            port,
            worker_prefix,
            tmux_socket,
            tmux_program,
        }
    }

    /// Look the worker up and plan its attach. `WorkerList` is used rather
    /// than `WorkerInfo` because it carries the full worker id, which names
    /// a local worker's session; the newest running row for the id wins, as
    /// with `worker stop`.
    pub async fn resolve(&self, worker_id: &str) -> Result<AttachPlan> {
        let channel = crate::connection::connect(self.port).await?;
        let workers = CoreServiceClient::new(channel)
            .worker_list(WorkerListRequest {})
            .await?
            .into_inner()
            .workers;
        Ok(self.plan_for(select_worker(&workers, worker_id)?))
    }

    pub fn plan_for(&self, worker: &WorkerSummary) -> AttachPlan {
        if worker.runtime == ur_rpc::runtime::LOCAL {
            AttachPlan::Local {
                worker_id: worker.worker_id.clone(),
                session: tmux::Session::on_socket(
                    &self.tmux_socket,
                    &format!("ur-{}", worker.worker_id_full),
                ),
            }
        } else {
            AttachPlan::Container {
                container: ContainerId(format!("{}{}", self.worker_prefix, worker.worker_id)),
                command: tmux::Session::from_name(CONTAINER_SESSION).attach_command(),
            }
        }
    }

    /// Wait until the freshly launched worker can be attached to.
    ///
    /// Local workers need no wait: the launch RPC returns only after builderd
    /// created the tmux session, and [`Self::attach`] checks it still exists.
    pub async fn wait_ready(&self, plan: &AttachPlan) -> Result<()> {
        match plan {
            AttachPlan::Container { container, .. } => {
                let container = container.clone();
                tokio::task::spawn_blocking(move || wait_for_healthy(&container))
                    .await
                    .context("health wait task panicked")?
            }
            AttachPlan::Local { .. } => Ok(()),
        }
    }

    /// Attach interactively; returns the attach's exit code.
    pub async fn attach(&self, plan: &AttachPlan) -> Result<i32> {
        match plan.clone() {
            AttachPlan::Container { container, command } => {
                info!(container = %container.0, "attaching to agent session");
                tokio::task::spawn_blocking(move || {
                    let status =
                        container::runtime_from_env().exec_interactive(&container, &command)?;
                    Ok(status.code().unwrap_or(1))
                })
                .await
                .context("attach task panicked")?
            }
            AttachPlan::Local { worker_id, session } => {
                if !session.exists().await? {
                    bail!(
                        "local worker {worker_id} has no tmux session '{}' on socket '{}' \
                         (the agent may have exited); run `ur worker list`",
                        session.name(),
                        session.socket().unwrap_or_default()
                    );
                }
                info!(
                    worker_id,
                    session = session.name(),
                    "attaching to local agent session"
                );
                let mut cmd = self.local_attach_command(&session);
                tokio::task::spawn_blocking(move || {
                    let status = cmd.status().context("failed to exec tmux attach")?;
                    Ok(status.code().unwrap_or(1))
                })
                .await
                .context("attach task panicked")?
            }
        }
    }

    /// The host attach for a local session. `TMUX` is inherited on purpose:
    /// attaching from the user's own tmux (another server) works with it set,
    /// and it keeps tmux's guard against nesting the `ur` server in itself.
    fn local_attach_command(&self, session: &tmux::Session) -> Command {
        let argv = session.attach_command();
        let mut cmd = Command::new(&self.tmux_program);
        cmd.args(&argv[1..]);
        cmd
    }

    /// Attach for `--rm`, then run `stop` once the attach ends.
    ///
    /// Acts like a shell `trap`: SIGHUP/SIGINT/SIGTERM (e.g. closing the
    /// terminal tab) also fall through to `stop`, so no worker is orphaned.
    pub async fn attach_with_rm<F, Fut>(&self, plan: &AttachPlan, stop: F) -> Result<i32>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<()>>,
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut sighup = signal(SignalKind::hangup()).context("install SIGHUP handler")?;
        let mut sigint = signal(SignalKind::interrupt()).context("install SIGINT handler")?;
        let mut sigterm = signal(SignalKind::terminate()).context("install SIGTERM handler")?;

        let exit_code = tokio::select! {
            res = self.attach(plan) => res?,
            _ = sighup.recv() => {
                eprintln!("\nTerminal closed; stopping worker (--rm)...");
                129 // 128 + SIGHUP(1)
            }
            _ = sigint.recv() => {
                eprintln!("\nInterrupted; stopping worker (--rm)...");
                130 // 128 + SIGINT(2)
            }
            _ = sigterm.recv() => {
                eprintln!("\nTerminated; stopping worker (--rm)...");
                143 // 128 + SIGTERM(15)
            }
        };
        stop().await?;
        Ok(exit_code)
    }
}

/// The newest running row for `worker_id` (`WorkerList` lists oldest first
/// per process id, like `worker stop` relies on).
fn select_worker<'a>(workers: &'a [WorkerSummary], worker_id: &str) -> Result<&'a WorkerSummary> {
    workers
        .iter()
        .rev()
        .find(|w| w.worker_id == worker_id)
        .with_context(|| {
            format!("unknown worker: {worker_id} (run `ur worker list` to see running workers)")
        })
}

/// Wait for a container's Docker HEALTHCHECK to report "healthy".
/// Polls every 500ms for up to 60s.
fn wait_for_healthy(id: &ContainerId) -> Result<()> {
    let runtime = container::runtime_from_env();
    let max_attempts = 120; // 60s at 500ms intervals
    let mut printed = false;
    for i in 0..max_attempts {
        let status = runtime.health_status(id).unwrap_or_default();
        if status == "healthy" {
            if printed {
                eprintln!(" ready");
            }
            return Ok(());
        }
        if status == "unhealthy" {
            if printed {
                eprintln!();
            }
            bail!("container {} became unhealthy", id.0);
        }
        if i == 0 {
            eprint!("Waiting for worker to initialize");
            printed = true;
        }
        eprint!(".");
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    if printed {
        eprintln!();
    }
    bail!("container {} did not become healthy after 60s", id.0);
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;

    use super::*;

    /// A tmux socket no other test (or the real `ur` socket) uses.
    fn unique_socket() -> String {
        static N: AtomicU32 = AtomicU32::new(0);
        format!(
            "ur-attach-test-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// A stand-in `tmux` that records its argv and exits 3, so a test never
    /// grabs the real terminal.
    fn fake_tmux(dir: &std::path::Path) -> PathBuf {
        let path = dir.join("tmux");
        let script = format!(
            "#!/bin/sh\necho \"$@\" > '{}'\nexit 3\n",
            dir.join("args").display()
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn manager(socket: &str, program: PathBuf) -> AttachManager {
        AttachManager::new(0, "ur-worker-".into(), socket.into(), program)
    }

    fn summary(runtime: &str) -> WorkerSummary {
        WorkerSummary {
            worker_id: "w1".into(),
            worker_id_full: "w1-ab12".into(),
            runtime: runtime.into(),
            ..Default::default()
        }
    }

    fn local_plan(socket: &str) -> AttachPlan {
        AttachPlan::Local {
            worker_id: "w1".into(),
            session: tmux::Session::on_socket(socket, "ur-w1-ab12"),
        }
    }

    #[test]
    fn container_worker_attaches_via_container_exec() {
        let m = manager("ur", "tmux".into());
        for runtime in [ur_rpc::runtime::CONTAINER, ""] {
            let AttachPlan::Container { container, command } = m.plan_for(&summary(runtime)) else {
                panic!("runtime {runtime:?} must attach through the container");
            };
            assert_eq!(container.0, "ur-worker-w1");
            assert_eq!(command, ["tmux", "-u", "attach-session", "-t", "=agent"]);
        }
    }

    #[test]
    fn local_worker_attaches_via_host_tmux_socket() {
        let m = manager("ur", "tmux".into());
        let AttachPlan::Local { worker_id, session } = m.plan_for(&summary(ur_rpc::runtime::LOCAL))
        else {
            panic!("local runtime must attach on the host");
        };
        assert_eq!(worker_id, "w1");
        assert_eq!(
            session.attach_command(),
            [
                "tmux",
                "-L",
                "ur",
                "-u",
                "attach-session",
                "-t",
                "=ur-w1-ab12"
            ]
        );
    }

    #[tokio::test]
    async fn rm_on_local_worker_stops_after_attach_exits_without_health_wait() {
        let socket = unique_socket();
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&socket, fake_tmux(dir.path()));
        let session = tmux::Session::create(tmux::CreateOptions {
            name: "ur-w1-ab12".into(),
            socket: Some(socket.clone()),
            detached: true,
            ..Default::default()
        })
        .await
        .unwrap();
        let plan = local_plan(&socket);

        // A Docker health wait would poll a nonexistent container for 60s.
        tokio::time::timeout(Duration::from_secs(5), m.wait_ready(&plan))
            .await
            .expect("local wait_ready must not poll Docker")
            .unwrap();

        let args = dir.path().join("args");
        let mut stopped_after_attach = None;
        let code = m
            .attach_with_rm(&plan, || async {
                stopped_after_attach = Some(args.exists());
                Ok(())
            })
            .await;
        session.kill().await.unwrap();

        assert_eq!(code.unwrap(), 3, "attach exit code is returned");
        assert_eq!(
            stopped_after_attach,
            Some(true),
            "stop runs after the attach exits"
        );
        assert_eq!(
            std::fs::read_to_string(&args).unwrap().trim(),
            format!("-L {socket} -u attach-session -t =ur-w1-ab12")
        );
    }

    #[test]
    fn select_worker_takes_newest_row_for_the_id() {
        let row = |id: &str, full: &str, runtime: &str| WorkerSummary {
            worker_id: id.into(),
            worker_id_full: full.into(),
            runtime: runtime.into(),
            ..Default::default()
        };
        let workers = [
            row("w1", "w1-old1", ur_rpc::runtime::CONTAINER),
            row("w1", "w1-new1", ur_rpc::runtime::LOCAL),
            row("w2", "w2-aaaa", ur_rpc::runtime::CONTAINER),
        ];
        let picked = select_worker(&workers, "w1").unwrap();
        assert_eq!(picked.worker_id_full, "w1-new1");
        assert_eq!(picked.runtime, ur_rpc::runtime::LOCAL);

        let err = select_worker(&workers, "w3").unwrap_err().to_string();
        assert!(err.contains("unknown worker: w3"), "{err}");
    }

    /// `TMUX` must reach the attach: unsetting it disables tmux's refusal to
    /// nest the `ur` server inside one of its own panes.
    #[test]
    fn local_attach_inherits_tmux_env() {
        let m = manager("ur", "/fake/tmux".into());
        let cmd = m.local_attach_command(&tmux::Session::on_socket("ur", "ur-w1-ab12"));
        assert_eq!(cmd.get_program(), "/fake/tmux");
        let args: Vec<_> = cmd.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            ["-L", "ur", "-u", "attach-session", "-t", "=ur-w1-ab12"]
        );
        assert!(
            cmd.get_envs().all(|(k, _)| k != "TMUX"),
            "TMUX must be inherited unchanged"
        );
    }

    #[tokio::test]
    async fn attach_to_gone_local_session_is_a_clear_error() {
        let socket = unique_socket();
        let dir = tempfile::tempdir().unwrap();
        let m = manager(&socket, fake_tmux(dir.path()));

        let err = m
            .attach(&local_plan(&socket))
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("local worker w1"), "{err}");
        assert!(err.contains("ur worker list"), "{err}");
        assert!(
            !dir.path().join("args").exists(),
            "tmux attach must not run"
        );
    }
}
