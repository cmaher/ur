use std::os::unix::process::CommandExt;
use std::process::Stdio;

use anyhow::{Context, Result};
use fs4::fs_std::FileExt;
use tracing::{debug, info, instrument, warn};

use crate::output::OutputManager;

/// Resolve the builderd binary path. Looks next to the current executable first
/// (handles target/debug/ during development), then falls back to PATH lookup.
#[instrument]
fn builderd_bin() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("builderd");
        if sibling.exists() {
            debug!(path = %sibling.display(), "found builderd next to current executable");
            return sibling;
        }
    }
    debug!("falling back to PATH lookup for builderd");
    std::path::PathBuf::from("builderd")
}

/// Check whether the given PID is a live builderd process. PID files survive
/// crashes and reboots, so a bare existence check (kill -0) can match an
/// unrelated process that reused the PID — `start_builderd` would then short-
/// circuit and no builderd would actually be running.
fn is_builderd_running(pid: u32) -> bool {
    process_command_name(pid).as_deref() == Some("builderd")
}

/// Return the command basename for a PID, or `None` if the process doesn't
/// exist. Uses `ps -o comm=` for cross-platform support (Linux returns the
/// basename, macOS returns the full path — `Path::file_name` normalizes).
fn process_command_name(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if raw.is_empty() {
        return None;
    }
    std::path::Path::new(&raw)
        .file_name()
        .and_then(|s| s.to_str())
        .map(str::to_owned)
}

#[instrument(skip(config, output), fields(builderd_port = config.builderd_port))]
pub fn start_builderd(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::BUILDERD_PID_FILE);

    // Take an exclusive lock on a lock file to prevent races between concurrent
    // `ur start` invocations. The lock is held only while we check + spawn.
    let lock_path = config.config_dir.join("builderd.lock");
    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&lock_path)
        .context("failed to open builderd lock file")?;
    lock_file
        .lock_exclusive()
        .context("failed to acquire builderd lock")?;

    // Check for existing process (under lock so no race)
    if pid_file.exists() {
        let pid_str = std::fs::read_to_string(&pid_file)?;
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            if is_builderd_running(pid) {
                info!(pid, "builderd already running");
                output.print_text(&format!("builderd already running (pid {pid})"));
                return Ok(());
            }
            let other = process_command_name(pid);
            info!(pid, ?other, "removing stale builderd PID file");
            std::fs::remove_file(&pid_file)?;
        }
    }

    let bin = builderd_bin();
    debug!(bin = %bin.display(), "spawning builderd");

    // Redirect stderr to a file so panics are captured but the daemon doesn't
    // hold the parent's stderr pipe open (which would block callers using
    // `Command::output()` to capture ur's output).
    std::fs::create_dir_all(&config.logs_dir).context("failed to create logs directory")?;
    let stderr_path = config.logs_dir.join("builderd.err");
    let stderr_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&stderr_path)
        .context("failed to open builderd stderr log")?;

    let args = builderd_args(config);
    let addr = builderd_dial_addr(&args, config.builderd_port);

    let mut child = std::process::Command::new(&bin)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr_file))
        // Put builderd in its own process group so signals sent to the ur CLI
        // (e.g. Ctrl-C) don't propagate to the daemon.
        .process_group(0)
        .spawn()
        .context("failed to spawn builderd — is it installed and on PATH?")?;

    let pid = child.id();
    std::fs::write(&pid_file, pid.to_string())?;
    // builderd adopts surviving local workers before it listens, and exits
    // if it cannot; only a listening port means it is up.
    if let Err(e) = wait_for_builderd(&mut child, addr, BUILDERD_START_TIMEOUT) {
        let _ = child.kill();
        let _ = std::fs::remove_file(&pid_file);
        anyhow::bail!(
            "builderd failed to start: {e}; see {} and {}",
            config.logs_dir.join("builderd.log*").display(),
            stderr_path.display()
        );
    }
    info!(pid, "builderd started");
    output.print_text(&format!("builderd started (pid {pid})"));

    // Lock is released when lock_file is dropped
    Ok(())
}

/// How long `ur start` waits for builderd to listen.
const BUILDERD_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Where the host dials builderd: its `--bind` address, else loopback.
fn builderd_dial_addr(args: &[String], port: u16) -> std::net::SocketAddr {
    let ip = args
        .iter()
        .position(|a| a == "--bind")
        .and_then(|i| args.get(i + 1))
        .and_then(|ip| ip.parse().ok())
        .unwrap_or(std::net::IpAddr::from([127, 0, 0, 1]));
    std::net::SocketAddr::new(ip, port)
}

/// Wait until `addr` accepts a TCP connection, failing early if `child`
/// exits and after `timeout`.
fn wait_for_builderd(
    child: &mut std::process::Child,
    addr: std::net::SocketAddr,
    timeout: std::time::Duration,
) -> Result<()> {
    let poll = std::time::Duration::from_millis(100);
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("builderd exited ({status})");
        }
        if std::net::TcpStream::connect_timeout(&addr, poll).is_ok() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("builderd did not listen on {addr} within {timeout:?}");
        }
        std::thread::sleep(poll);
    }
}

/// Whether a live builderd owns the PID file.
pub fn builderd_running(config: &ur_config::Config) -> bool {
    std::fs::read_to_string(config.config_dir.join(ur_config::BUILDERD_PID_FILE))
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(is_builderd_running)
}

/// builderd's gRPC endpoint as the host dials it.
pub fn builderd_endpoint(config: &ur_config::Config) -> String {
    let addr = builderd_dial_addr(&builderd_args(config), config.builderd_port);
    format!("http://{addr}")
}

/// `ur stop`: tear down every local worker builderd hosts, whatever its row
/// says (rows converge on the next server start). `endpoint` is `None` when
/// no builderd is running: surviving sessions are adopted on the next start.
/// Any list or stop failure is an error, so `ur stop` keeps builderd up.
pub async fn stop_local_workers(endpoint: Option<String>, output: &OutputManager) -> Result<()> {
    use ur_rpc::proto::builder_local_worker::builder_local_worker_service_client::BuilderLocalWorkerServiceClient;
    use ur_rpc::proto::builder_local_worker::{ListLocalWorkersRequest, StopLocalWorkerRequest};

    let Some(endpoint) = endpoint else {
        warn!("builderd not running; local worker sessions, if any, are adopted on next start");
        output.print_text(
            "Warning: builderd is not running; any surviving local worker sessions will be adopted on the next start",
        );
        return Ok(());
    };
    let mut client = BuilderLocalWorkerServiceClient::connect(endpoint.clone())
        .await
        .with_context(|| format!("connect to builderd at {endpoint}"))?;
    let workers = client
        .list_local_workers(ListLocalWorkersRequest {})
        .await
        .map_err(|s| anyhow::anyhow!("list local workers: {}", s.message()))?
        .into_inner()
        .workers;
    let mut failed = Vec::new();
    for w in workers {
        match client
            .stop_local_worker(StopLocalWorkerRequest {
                worker_id: w.worker_id.clone(),
            })
            .await
        {
            Ok(_) => output.print_text(&format!("Stopped local worker {}", w.worker_id)),
            Err(s) => failed.push(format!("{}: {}", w.worker_id, s.message())),
        }
    }
    if !failed.is_empty() {
        anyhow::bail!(
            "failed to stop local workers; server and builderd left running:\n{}",
            failed.join("\n")
        );
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn builderd_args(config: &ur_config::Config) -> Vec<String> {
    vec![
        "--port".to_string(),
        config.builderd_port.to_string(),
        "--workspace".to_string(),
        config.workspace.display().to_string(),
        "--logs-dir".to_string(),
        config.logs_dir.display().to_string(),
        "--config-dir".to_string(),
        config.config_dir.display().to_string(),
    ]
}

#[cfg(not(target_os = "macos"))]
fn builderd_args(config: &ur_config::Config) -> Vec<String> {
    let mut args = vec![
        "--port".to_string(),
        config.builderd_port.to_string(),
        "--workspace".to_string(),
        config.workspace.display().to_string(),
        "--logs-dir".to_string(),
        config.logs_dir.display().to_string(),
        "--config-dir".to_string(),
        config.config_dir.display().to_string(),
    ];

    // Bind builderd to the Docker bridge gateway IP so containers can reach it
    // via host.docker.internal. On macOS, Docker Desktop handles this natively.
    if let Some(gateway_ip) = detect_docker_bridge_ip() {
        info!(gateway_ip, "binding builderd to Docker bridge gateway");
        args.extend(["--bind".to_string(), gateway_ip]);
    } else {
        warn!(
            "could not detect Docker bridge IP; builderd will bind to 127.0.0.1 — containers may not be able to reach it"
        );
    }

    args
}

/// Detect the IP address of the Docker bridge network (`docker0`) by parsing
/// the output of `ip -4 addr show docker0`. Returns `None` if the interface
/// doesn't exist or the IP can't be parsed.
#[cfg(not(target_os = "macos"))]
fn detect_docker_bridge_ip() -> Option<String> {
    let output = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show", "docker0"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // Format: "N: docker0    inet 172.17.0.1/16 ..."
    let stdout = String::from_utf8_lossy(&output.stdout);
    let ip = stdout
        .split_whitespace()
        .skip_while(|s| *s != "inet")
        .nth(1)?
        .split('/')
        .next()?
        .to_string();
    debug!(ip, "detected Docker bridge IP");
    Some(ip)
}

#[instrument(skip(config, output))]
pub fn stop_builderd(config: &ur_config::Config, output: &OutputManager) -> Result<()> {
    let pid_file = config.config_dir.join(ur_config::BUILDERD_PID_FILE);

    if !pid_file.exists() {
        debug!("no builderd PID file found, nothing to stop");
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_file)?;
    if let Ok(pid) = pid_str.trim().parse::<u32>() {
        debug!(pid, "sending SIGTERM to builderd");
        let result = std::process::Command::new("kill")
            .arg(pid.to_string())
            .output();
        if let Err(e) = result {
            warn!(pid, error = %e, "failed to send SIGTERM to builderd");
        }
    }

    std::fs::remove_file(&pid_file)?;
    info!("builderd stopped");
    output.print_text("builderd stopped");

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tonic::{Request, Response, Status};
    use ur_rpc::proto::builder_local_worker::builder_local_worker_service_server::{
        BuilderLocalWorkerService, BuilderLocalWorkerServiceServer,
    };
    use ur_rpc::proto::builder_local_worker::{
        LaunchLocalWorkerRequest, LaunchLocalWorkerResponse, ListLocalWorkersRequest,
        ListLocalWorkersResponse, LocalWorkerStatus, StopLocalWorkerRequest,
        StopLocalWorkerResponse,
    };

    use super::*;

    fn sh(script: &str) -> std::process::Child {
        std::process::Command::new("sh")
            .args(["-c", script])
            .spawn()
            .unwrap()
    }

    #[test]
    fn wait_for_builderd_reports_exit_timeout_and_listening() {
        let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = free.local_addr().unwrap();
        drop(free);
        let timeout = std::time::Duration::from_secs(5);

        let err = wait_for_builderd(&mut sh("exit 3"), addr, timeout).unwrap_err();
        assert!(err.to_string().contains("exited"), "{err}");

        let mut child = sh("sleep 5");
        let err =
            wait_for_builderd(&mut child, addr, std::time::Duration::from_millis(300)).unwrap_err();
        assert!(err.to_string().contains("did not listen"), "{err}");

        let listener = std::net::TcpListener::bind(addr).unwrap();
        wait_for_builderd(&mut child, addr, timeout).unwrap();
        drop(listener);
        let _ = child.kill();
    }

    #[test]
    fn dial_addr_uses_bind_or_loopback() {
        assert_eq!(builderd_dial_addr(&[], 7).to_string(), "127.0.0.1:7");
        let args = ["--bind".to_owned(), "172.17.0.1".to_owned()];
        assert_eq!(builderd_dial_addr(&args, 7).to_string(), "172.17.0.1:7");
    }

    #[derive(Clone, Default)]
    struct FakeBuilderd {
        listed: Vec<String>,
        list_error: bool,
        stop_error_for: Option<String>,
        stops: Arc<Mutex<Vec<String>>>,
    }

    #[tonic::async_trait]
    impl BuilderLocalWorkerService for FakeBuilderd {
        async fn launch_local_worker(
            &self,
            _: Request<LaunchLocalWorkerRequest>,
        ) -> Result<Response<LaunchLocalWorkerResponse>, Status> {
            Err(Status::unimplemented("launch"))
        }

        async fn stop_local_worker(
            &self,
            req: Request<StopLocalWorkerRequest>,
        ) -> Result<Response<StopLocalWorkerResponse>, Status> {
            let id = req.into_inner().worker_id;
            self.stops.lock().unwrap().push(id.clone());
            if self.stop_error_for.as_deref() == Some(id.as_str()) {
                return Err(Status::internal("kill tmux session: boom"));
            }
            Ok(Response::new(StopLocalWorkerResponse {}))
        }

        async fn list_local_workers(
            &self,
            _: Request<ListLocalWorkersRequest>,
        ) -> Result<Response<ListLocalWorkersResponse>, Status> {
            if self.list_error {
                return Err(Status::internal("check tmux session: boom"));
            }
            let workers = self
                .listed
                .iter()
                .map(|id| LocalWorkerStatus {
                    worker_id: id.clone(),
                    session_alive: true,
                })
                .collect();
            Ok(Response::new(ListLocalWorkersResponse { workers }))
        }
    }

    async fn serve(fake: FakeBuilderd) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        tokio::spawn(
            tonic::transport::Server::builder()
                .add_service(BuilderLocalWorkerServiceServer::new(fake))
                .serve_with_incoming(incoming),
        );
        format!("http://{addr}")
    }

    /// `ur stop` stops every listed local worker; a list or stop failure is
    /// an error (shutdown aborts); no builderd only warns.
    #[tokio::test]
    async fn stop_local_workers_gates_shutdown() {
        let output = OutputManager::from_args(None);
        stop_local_workers(None, &output).await.unwrap();

        let ok = FakeBuilderd {
            listed: vec!["w1".into(), "w2".into()],
            ..Default::default()
        };
        stop_local_workers(Some(serve(ok.clone()).await), &output)
            .await
            .unwrap();
        assert_eq!(*ok.stops.lock().unwrap(), vec!["w1", "w2"]);

        let list_fails = FakeBuilderd {
            listed: vec!["w1".into()],
            list_error: true,
            ..Default::default()
        };
        let err = stop_local_workers(Some(serve(list_fails.clone()).await), &output)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("list local workers"), "{err}");
        assert!(list_fails.stops.lock().unwrap().is_empty());

        let stop_fails = FakeBuilderd {
            listed: vec!["w1".into(), "w2".into()],
            stop_error_for: Some("w1".into()),
            ..Default::default()
        };
        let err = stop_local_workers(Some(serve(stop_fails.clone()).await), &output)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("w1: kill tmux session: boom"),
            "{err}"
        );
        assert_eq!(*stop_fails.stops.lock().unwrap(), vec!["w1", "w2"]);
    }
}
