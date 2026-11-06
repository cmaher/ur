use std::path::PathBuf;
use std::time::{Duration, Instant};

use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use crate::barrier::WorkerBarrier;
use container::{
    ContainerId, ContainerRuntime, DockerRuntime, ExecOpts, ImageId, PortMap, RunOpts,
    runtime_from_env,
};
use ur_rpc::proto::builder_container::builder_container_service_server::BuilderContainerService;
use ur_rpc::proto::builder_container::{
    ExecContainerRequest, ExecContainerResponse, InspectNetworkRequest, InspectNetworkResponse,
    LaunchWorkerRequest, LaunchWorkerResponse, ListWorkerContainersRequest,
    ListWorkerContainersResponse, StopWorkerRequest, StopWorkerResponse, WorkerContainerStatus,
};

/// Label carrying the worker id of every worker container.
pub const WORKER_ID_LABEL: &str = "ur.worker_id";
/// Label scoping worker containers to one ur instance (its builderd).
pub const INSTANCE_LABEL: &str = "ur.instance";

/// Handles BuilderContainerService RPCs: launch, stop, list, exec, and
/// network inspect for worker containers. Uses DockerRuntime from the
/// container crate. A worker container is identified only by its labels:
/// `ur.worker_id` and `ur.instance` (this builderd's instance).
#[derive(Clone)]
pub struct BuilderContainerHandler {
    runtime: DockerRuntime,
    /// Orders LaunchWorker and StopWorker per worker id (Stop is a barrier).
    barrier: WorkerBarrier,
    /// This ur instance's `ur.instance` label value; containers without it
    /// (another instance's) are never listed or stopped.
    instance: String,
}

impl BuilderContainerHandler {
    pub fn new(barrier: WorkerBarrier, instance: String) -> Self {
        Self {
            runtime: runtime_from_env(),
            barrier,
            instance,
        }
    }
}

/// Check whether a docker "No such container" message appears in the given text.
fn is_no_such_container(text: &str) -> bool {
    text.contains("No such container")
}

/// Convert a `LaunchWorkerRequest` into `RunOpts` for the container runtime,
/// labeled with the worker id and `instance`.
fn request_to_run_opts(req: &LaunchWorkerRequest, instance: &str) -> RunOpts {
    let volumes = req
        .volumes
        .iter()
        .map(|v| {
            (
                PathBuf::from(&v.host_path),
                PathBuf::from(&v.container_path),
            )
        })
        .collect();

    let port_maps = req
        .port_maps
        .iter()
        .map(|p| PortMap {
            host_port: p.host_port as u16,
            container_port: p.container_port as u16,
        })
        .collect();

    let env_vars = req
        .env_vars
        .iter()
        .map(|e| (e.key.clone(), e.value.clone()))
        .collect();

    let workdir = if req.workdir.is_empty() {
        None
    } else {
        Some(PathBuf::from(&req.workdir))
    };

    let network = if req.network.is_empty() {
        None
    } else {
        Some(req.network.clone())
    };

    let add_hosts = req
        .add_hosts
        .iter()
        .map(|a| (a.host.clone(), a.ip.clone()))
        .collect();

    RunOpts {
        image: ImageId(req.image.clone()),
        name: req.name.clone(),
        cpus: req.cpus,
        memory: req.memory.clone(),
        volumes,
        port_maps,
        env_vars,
        workdir,
        command: vec![],
        network,
        add_hosts,
        labels: vec![
            (WORKER_ID_LABEL.to_owned(), req.worker_id.clone()),
            (INSTANCE_LABEL.to_owned(), instance.to_owned()),
        ],
    }
}

/// Stat each volume host path; return `Err` with the missing path on first failure.
fn check_volume_sources(req: &LaunchWorkerRequest) -> Result<(), String> {
    for vol in &req.volumes {
        let path = PathBuf::from(&vol.host_path);
        if let Err(e) = std::fs::metadata(&path) {
            return Err(format!("{}: {e}", path.display()));
        }
    }
    Ok(())
}

enum StartupState {
    Starting,
    Ready,
    Exited { logs: String },
}

fn wait_for_worker_startup(
    timeout: Duration,
    poll_interval: Duration,
    mut probe: impl FnMut() -> Result<StartupState, String>,
) -> Result<(), String> {
    let started = Instant::now();
    loop {
        match probe()? {
            StartupState::Ready => return Ok(()),
            StartupState::Exited { logs } => {
                return Err(format!("worker container exited during startup: {logs}"));
            }
            StartupState::Starting if started.elapsed() >= timeout => {
                return Err(format!(
                    "timed out after {}s waiting for worker startup",
                    timeout.as_secs()
                ));
            }
            StartupState::Starting => {
                std::thread::sleep(poll_interval);
            }
        }
    }
}

fn probe_worker_startup(
    runtime: &impl ContainerRuntime,
    container_id: &ContainerId,
) -> Result<StartupState, String> {
    let state = runtime
        .inspect_state(container_id)
        .map_err(|error| format!("failed to inspect worker during startup: {error}"))?;
    if !state.is_some_and(|state| state.running) {
        let logs = runtime
            .logs(container_id)
            .unwrap_or_else(|error| format!("failed to read container logs: {error}"));
        return Ok(StartupState::Exited { logs });
    }

    let health = runtime
        .health_status(container_id)
        .map_err(|error| format!("failed to inspect worker health during startup: {error}"))?;
    match health.as_str() {
        "healthy" | "" => Ok(StartupState::Ready),
        "unhealthy" => Ok(StartupState::Starting),
        _ => Ok(StartupState::Starting),
    }
}

fn cleanup_failed_launch(
    runtime: &impl ContainerRuntime,
    container_id: &ContainerId,
) -> Result<(), String> {
    let mut errors = Vec::new();
    match runtime.inspect_state(container_id) {
        Ok(Some(state)) if state.running => {
            if let Err(error) = runtime.stop(container_id) {
                errors.push(format!("failed to stop container: {error}"));
            }
        }
        Ok(_) => {}
        Err(error) => errors.push(format!("failed to inspect container for cleanup: {error}")),
    }
    if let Err(error) = runtime.rm(container_id) {
        errors.push(format!("failed to remove container: {error}"));
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

impl BuilderContainerHandler {
    /// `docker run` plus the startup wait (the old LaunchWorker body).
    async fn launch_container(
        &self,
        req: LaunchWorkerRequest,
    ) -> Result<Response<LaunchWorkerResponse>, Status> {
        info!(
            image = %req.image,
            name = %req.name,
            volume_count = req.volumes.len(),
            "LaunchWorker request received"
        );

        // Stat each volume source before attempting docker run.
        if let Err(missing) = check_volume_sources(&req) {
            warn!(missing = %missing, "volume source missing for LaunchWorker");
            return Err(Status::failed_precondition(format!(
                "volume source path does not exist: {missing}"
            )));
        }

        let opts = request_to_run_opts(&req, &self.instance);
        let container_id = self.runtime.run(&opts).map_err(|e| {
            error!(error = %e, name = %req.name, "docker run failed");
            Status::internal(format!("docker run failed: {e}"))
        })?;

        let startup_runtime = self.runtime.clone();
        let startup_container_id = container_id.clone();
        let startup_result = tokio::task::spawn_blocking(move || {
            wait_for_worker_startup(Duration::from_secs(60), Duration::from_millis(100), || {
                probe_worker_startup(&startup_runtime, &startup_container_id)
            })
        })
        .await
        .map_err(|error| format!("worker startup check failed: {error}"))
        .and_then(|result| result);
        if let Err(error) = startup_result {
            let cleanup = cleanup_failed_launch(&self.runtime, &container_id).map_or_else(
                |cleanup| format!("; cleanup also failed: {cleanup}"),
                |()| String::new(),
            );
            return Err(Status::failed_precondition(format!("{error}{cleanup}")));
        }

        info!(container_id = %container_id.0, name = %req.name, "container launched");
        Ok(Response::new(LaunchWorkerResponse {}))
    }

    /// `docker stop` + `rm` of one container; a container already gone is Ok.
    fn remove_container(&self, id: &ContainerId) -> Result<(), Status> {
        let gone = |op: &str, e: anyhow::Error| {
            let msg = e.to_string();
            if is_no_such_container(&msg) {
                return Ok(());
            }
            error!(container_id = %id.0, error = %msg, "docker {op} failed");
            Err(Status::internal(format!("docker {op} failed: {msg}")))
        };
        if let Err(e) = self.runtime.stop(id) {
            return gone("stop", e);
        }
        if let Err(e) = self.runtime.rm(id) {
            return gone("rm", e);
        }
        info!(container_id = %id.0, "container stopped and removed");
        Ok(())
    }

    /// This instance's containers labeled with `worker_id`.
    fn containers_of(&self, worker_id: &str) -> Result<Vec<ContainerId>, Status> {
        self.runtime
            .list_by_labels(
                &[
                    (WORKER_ID_LABEL, worker_id),
                    (INSTANCE_LABEL, &self.instance),
                ],
                WORKER_ID_LABEL,
            )
            .map(|found| found.into_iter().map(|c| c.id).collect())
            .map_err(|e| Status::internal(format!("list worker containers: {e}")))
    }

    /// Upgrade only: a pre-label container is stopped only when its
    /// `UR_WORKER_ID` env names `worker_id` (container names were reused).
    fn legacy_container_of(
        &self,
        worker_id: &str,
        legacy: &str,
    ) -> Result<Option<ContainerId>, Status> {
        if legacy.is_empty() {
            return Ok(None);
        }
        let id = ContainerId(legacy.to_owned());
        let env = self
            .runtime
            .env_of(&id)
            .map_err(|e| Status::internal(format!("inspect legacy container: {e}")))?;
        let owned = env.is_some_and(|env| {
            env.iter()
                .any(|e| *e == format!("{}={worker_id}", ur_config::UR_WORKER_ID_ENV))
        });
        Ok(owned.then_some(id))
    }

    /// Stop as a barrier for `req.worker_id`: tombstone it, wait for its
    /// in-flight launch, then remove every container labeled with it.
    async fn stop_worker_barrier(
        &self,
        req: StopWorkerRequest,
    ) -> Result<Response<StopWorkerResponse>, Status> {
        info!(worker_id = %req.worker_id, "StopWorker request received");
        let _stopping = self.barrier.begin_stop(&req.worker_id).await;
        let mut targets = self.containers_of(&req.worker_id)?;
        targets.extend(self.legacy_container_of(&req.worker_id, &req.legacy_container_id)?);
        for id in &targets {
            self.remove_container(id)?;
        }
        Ok(Response::new(StopWorkerResponse {}))
    }
}

#[tonic::async_trait]
impl BuilderContainerService for BuilderContainerHandler {
    /// Runs on its own task under the worker's barrier lock, so a cancelled
    /// RPC cannot abandon it and a StopWorker of the worker is ordered
    /// against it.
    async fn launch_worker(
        &self,
        req: Request<LaunchWorkerRequest>,
    ) -> Result<Response<LaunchWorkerResponse>, Status> {
        let (this, req) = (self.clone(), req.into_inner());
        if req.worker_id.is_empty() {
            return Err(Status::invalid_argument("LaunchWorker requires worker_id"));
        }
        tokio::spawn(async move {
            let _launching = this.barrier.begin_launch(&req.worker_id).await?;
            this.launch_container(req).await
        })
        .await
        .map_err(|e| Status::internal(format!("launch task failed: {e}")))?
    }

    async fn stop_worker(
        &self,
        req: Request<StopWorkerRequest>,
    ) -> Result<Response<StopWorkerResponse>, Status> {
        let req = req.into_inner();
        if req.worker_id.is_empty() {
            return Err(Status::invalid_argument("StopWorker requires worker_id"));
        }
        self.stop_worker_barrier(req).await
    }

    async fn list_worker_containers(
        &self,
        _req: Request<ListWorkerContainersRequest>,
    ) -> Result<Response<ListWorkerContainersResponse>, Status> {
        let containers = self
            .runtime
            .list_by_labels(&[(INSTANCE_LABEL, &self.instance)], WORKER_ID_LABEL)
            .map_err(|e| Status::internal(format!("list worker containers: {e}")))?
            .into_iter()
            .filter(|c| !c.label.is_empty())
            .map(|c| WorkerContainerStatus {
                worker_id: c.label,
                running: c.running,
            })
            .collect();
        Ok(Response::new(ListWorkerContainersResponse { containers }))
    }

    async fn exec_container(
        &self,
        req: Request<ExecContainerRequest>,
    ) -> Result<Response<ExecContainerResponse>, Status> {
        let req = req.into_inner();
        let id = ContainerId(req.container_id.clone());

        let mut command = vec![req.command.clone()];
        command.extend(req.args.iter().cloned());

        let workdir = if req.workdir.is_empty() {
            None
        } else {
            Some(PathBuf::from(&req.workdir))
        };

        info!(
            container_id = %req.container_id,
            command = %req.command,
            arg_count = req.args.len(),
            "ExecContainer request received"
        );

        let opts = ExecOpts { command, workdir };
        let output = self.runtime.exec(&id, &opts).map_err(|e| {
            error!(container_id = %req.container_id, error = %e, "docker exec failed");
            Status::internal(format!("docker exec failed: {e}"))
        })?;

        Ok(Response::new(ExecContainerResponse {
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
        }))
    }

    async fn inspect_network(
        &self,
        req: Request<InspectNetworkRequest>,
    ) -> Result<Response<InspectNetworkResponse>, Status> {
        let req = req.into_inner();

        info!(network = %req.name, "InspectNetwork request received");

        // Use the runtime's docker command to inspect the network.
        // An absent network returns exists: false — not an error.
        let output = std::process::Command::new(&self.runtime.command)
            .args(["network", "inspect", &req.name])
            .output()
            .map_err(|e| {
                error!(network = %req.name, error = %e, "failed to run docker network inspect");
                Status::internal(format!("failed to run docker network inspect: {e}"))
            })?;

        let exists = output.status.success();
        Ok(Response::new(InspectNetworkResponse { exists }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::time::Duration;
    use ur_rpc::proto::builder_container::{AddHost, EnvVar, Volume};

    fn make_request(volumes: Vec<Volume>) -> LaunchWorkerRequest {
        LaunchWorkerRequest {
            image: "ur-worker-claude:latest".into(),
            name: "test-container".into(),
            cpus: 2,
            memory: "4G".into(),
            volumes,
            port_maps: vec![],
            env_vars: vec![EnvVar {
                key: "FOO".into(),
                value: "bar".into(),
            }],
            workdir: "/workspace".into(),
            network: "ur".into(),
            add_hosts: vec![AddHost {
                host: "host.docker.internal".into(),
                ip: "172.17.0.1".into(),
            }],
            worker_id: "w1".into(),
        }
    }

    /// Fake `docker`: each container is `dstate/c/<name>/` holding `state`
    /// (`running`/`exited`), `labels` and `env` (one `k=v` per line) and
    /// `id` (`id-<name>`). Flag files in `dstate` inject health and stop
    /// failures; every successful `docker stop` appends the name to `KILLS`.
    const FAKE_DOCKER: &str = r#"#!/bin/bash
D="$(dirname "$0")/dstate"; mkdir -p "$D/c"
res() { for c in "$D"/c/*; do [ -d "$c" ] || continue; n=$(basename "$c")
  if [ "$n" = "$1" ] || [ "$(cat "$c/id")" = "$1" ]; then echo "$n"; return; fi; done; }
case "$1" in
  run) shift; name=""; labels=""; env=""
       while [ $# -gt 0 ]; do case "$1" in
         --name) name="$2"; shift;; --label) labels="$labels$2"$'\n'; shift;; -e) env="$env$2"$'\n'; shift;;
       esac; shift; done
       mkdir "$D/c/$name" 2>/dev/null || { echo "Conflict. The container name $name is already in use" >&2; exit 1; }
       echo running > "$D/c/$name/state"; printf '%s' "$labels" > "$D/c/$name/labels"
       printf '%s' "$env" > "$D/c/$name/env"; echo "id-$name" > "$D/c/$name/id"; echo "id-$name";;
  ps) shift; filters=""
      while [ $# -gt 0 ]; do [ "$1" = "--filter" ] && filters="$filters ${2#label=}"; shift; done
      for c in "$D"/c/*; do [ -d "$c" ] || continue; ok=1
        for f in $filters; do grep -qxF "$f" "$c/labels" || ok=0; done
        [ $ok = 1 ] || continue
        wid=$(grep '^ur.worker_id=' "$c/labels" | cut -d= -f2-); st=$(cat "$c/state")
        printf '%s\t%s\t%s\n' "$(cat "$c/id")" "$wid" "$st"; done;;
  inspect) n=$(res "$4"); [ -n "$n" ] || { echo "Error: No such container: $4" >&2; exit 1; }
       case "$3" in
         *Health*) [ -f "$D/HEALTH_DELAY" ] && sleep "$(cat "$D/HEALTH_DELAY")"
                   [ -f "$D/HEALTH_FAIL" ] && { echo "health boom" >&2; exit 1; }; echo "";;
         *Env*) cat "$D/c/$n/env";;
         *) if [ "$(cat "$D/c/$n/state")" = running ]; then echo "true $(cat "$D/c/$n/id")"; else echo "false $(cat "$D/c/$n/id")"; fi;;
       esac;;
  stop) n=$(res "$4"); [ -n "$n" ] || { echo "No such container: $4" >&2; exit 1; }
        [ -f "$D/STOP_FAIL" ] && { echo "stop boom" >&2; exit 1; }
        echo exited > "$D/c/$n/state"; echo "$n" >> "$D/KILLS";;
  rm) n=$(res "$2"); [ -n "$n" ] || { echo "No such container: $2" >&2; exit 1; }
      [ "$(cat "$D/c/$n/state")" = running ] && { echo "cannot remove a running container" >&2; exit 1; }
      rm -rf "$D/c/$n";;
  logs) echo logs;;
  *) echo "unsupported $*" >&2; exit 1;;
esac
"#;

    struct FakeDocker {
        dir: tempfile::TempDir,
    }

    impl FakeDocker {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().unwrap();
            let docker = dir.path().join("docker");
            std::fs::write(&docker, FAKE_DOCKER).unwrap();
            std::fs::set_permissions(&docker, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::create_dir_all(dir.path().join("dstate/c")).unwrap();
            Self { dir }
        }
        fn handler(&self, instance: &str) -> BuilderContainerHandler {
            BuilderContainerHandler {
                runtime: DockerRuntime {
                    command: self.dir.path().join("docker").display().to_string(),
                },
                barrier: WorkerBarrier::new(crate::barrier::TOMBSTONE_TTL),
                instance: instance.to_owned(),
            }
        }
        fn dstate(&self) -> std::path::PathBuf {
            self.dir.path().join("dstate")
        }
        fn flag(&self, name: &str, on: bool) {
            let path = self.dstate().join(name);
            if on {
                std::fs::write(path, "").unwrap();
            } else {
                let _ = std::fs::remove_file(path);
            }
        }
        /// `running`, `exited`, or `None` when the container is gone.
        fn state(&self, name: &str) -> Option<String> {
            std::fs::read_to_string(self.dstate().join("c").join(name).join("state"))
                .ok()
                .map(|s| s.trim().to_owned())
        }
        /// A container as a pre-ur-ezhdb launch left it: no labels.
        fn legacy(&self, name: &str, worker_id: &str) {
            let c = self.dstate().join("c").join(name);
            std::fs::create_dir_all(&c).unwrap();
            std::fs::write(c.join("state"), "running").unwrap();
            std::fs::write(c.join("labels"), "").unwrap();
            std::fs::write(c.join("env"), format!("UR_WORKER_ID={worker_id}\n")).unwrap();
            std::fs::write(c.join("id"), format!("id-{name}")).unwrap();
        }
    }

    fn launch_request(worker_id: &str) -> LaunchWorkerRequest {
        LaunchWorkerRequest {
            name: format!("ur-worker-{worker_id}"),
            worker_id: worker_id.into(),
            ..make_request(Vec::new())
        }
    }

    fn stop(worker_id: &str) -> Request<StopWorkerRequest> {
        Request::new(StopWorkerRequest {
            worker_id: worker_id.into(),
            legacy_container_id: String::new(),
        })
    }

    async fn listed(handler: &BuilderContainerHandler) -> Vec<(String, bool)> {
        let mut list: Vec<_> = handler
            .list_worker_containers(Request::new(ListWorkerContainersRequest {}))
            .await
            .unwrap()
            .into_inner()
            .containers
            .into_iter()
            .map(|c| (c.worker_id, c.running))
            .collect();
        list.sort();
        list
    }

    /// A StopWorker for a worker before its LaunchWorker arrives (the server
    /// stopped it mid-launch) makes the launch refuse, before any docker
    /// call; Ok from the stop means the worker will never run.
    #[tokio::test]
    async fn stop_before_launch_refuses_the_container_launch() {
        let docker = FakeDocker::new();
        let handler = docker.handler("inst-a");
        handler.stop_worker(stop("w1")).await.unwrap();
        let err = handler
            .launch_worker(Request::new(launch_request("w1")))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::FailedPrecondition);
        assert!(err.message().contains("was stopped"), "{err}");
        assert_eq!(docker.state("ur-worker-w1"), None, "docker run never ran");
    }

    /// Launch and Stop are keyed by worker id only: Launch requires one.
    #[tokio::test]
    async fn launch_and_stop_require_a_worker_id() {
        let handler = FakeDocker::new().handler("inst-a");
        let err = handler
            .launch_worker(Request::new(launch_request("")))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
        let err = handler.stop_worker(stop("")).await.unwrap_err();
        assert_eq!(err.code(), tonic::Code::InvalidArgument);
    }

    /// Launch labels the container with its worker id and this instance;
    /// List reports only this instance's containers, by worker id; Stop
    /// removes only the stopped worker's container.
    #[tokio::test(flavor = "multi_thread")]
    async fn list_and_stop_find_containers_by_worker_id_label_in_this_instance() {
        let docker = FakeDocker::new();
        let (mine, other) = (docker.handler("inst-a"), docker.handler("inst-b"));
        for id in ["w1", "w2"] {
            mine.launch_worker(Request::new(launch_request(id)))
                .await
                .unwrap();
        }
        other
            .launch_worker(Request::new(launch_request("w9")))
            .await
            .unwrap();
        assert_eq!(
            listed(&mine).await,
            vec![("w1".into(), true), ("w2".into(), true)]
        );

        mine.stop_worker(stop("w1")).await.unwrap();
        assert_eq!(docker.state("ur-worker-w1"), None);
        assert_eq!(docker.state("ur-worker-w2").as_deref(), Some("running"));
        mine.stop_worker(stop("w9")).await.unwrap();
        assert_eq!(
            docker.state("ur-worker-w9").as_deref(),
            Some("running"),
            "another instance's container is never stopped"
        );
        assert_eq!(listed(&mine).await, vec![("w2".into(), true)]);
        assert_eq!(listed(&other).await, vec![("w9".into(), true)]);
    }

    /// Upgrade: a pre-label container is stopped only when its env names the
    /// stopped worker (its process-id name was reused across launches).
    #[tokio::test]
    async fn legacy_container_is_stopped_only_when_its_env_names_the_worker() {
        let docker = FakeDocker::new();
        let handler = docker.handler("inst-a");
        docker.legacy("ur-worker-proj-1", "proj-1-new1");
        let legacy_stop = |worker_id: &str| {
            Request::new(StopWorkerRequest {
                worker_id: worker_id.into(),
                legacy_container_id: "id-ur-worker-proj-1".into(),
            })
        };
        handler
            .stop_worker(legacy_stop("proj-1-old0"))
            .await
            .unwrap();
        assert_eq!(docker.state("ur-worker-proj-1").as_deref(), Some("running"));
        handler
            .stop_worker(legacy_stop("proj-1-new1"))
            .await
            .unwrap();
        assert_eq!(docker.state("ur-worker-proj-1"), None);
        assert!(
            listed(&handler).await.is_empty(),
            "legacy containers are not listed"
        );
    }

    async fn wait_for_container(docker: &FakeDocker, name: &str) {
        for _ in 0..200 {
            if docker.state(name).is_some() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// ur-ezhdb #1: a launch fails after `docker run` and its cleanup fails
    /// too (container left running), while a Stop of the worker waits on the
    /// barrier. That Stop finds the leftover by its label: here it fails
    /// (docker stop fails), never returning Ok over a running container.
    #[tokio::test(flavor = "multi_thread")]
    async fn stop_waiting_on_a_failed_launch_targets_its_leftover_container() {
        let docker = FakeDocker::new();
        let handler = docker.handler("inst-a");
        std::fs::write(docker.dstate().join("HEALTH_DELAY"), "0.3").unwrap();
        docker.flag("HEALTH_FAIL", true);
        let launcher = handler.clone();
        let launch = tokio::spawn(async move {
            launcher
                .launch_worker(Request::new(launch_request("w1")))
                .await
        });
        wait_for_container(&docker, "ur-worker-w1").await;
        assert!(
            docker.state("ur-worker-w1").is_some(),
            "docker run happened"
        );
        docker.flag("STOP_FAIL", true);
        let stopper = handler.clone();
        let stop_task = tokio::spawn(async move { stopper.stop_worker(stop("w1")).await });

        let err = launch.await.unwrap().unwrap_err();
        assert!(err.message().contains("cleanup also failed"), "{err}");
        assert!(
            stop_task.await.unwrap().is_err(),
            "Stop returned Ok while the launched container is running"
        );
        assert_eq!(docker.state("ur-worker-w1").as_deref(), Some("running"));

        docker.flag("STOP_FAIL", false);
        handler.stop_worker(stop("w1")).await.unwrap();
        assert_eq!(
            docker.state("ur-worker-w1"),
            None,
            "the retried Stop removed it"
        );
    }

    #[test]
    fn request_to_run_opts_conversion() {
        let req = make_request(vec![Volume {
            host_path: "/tmp".into(),
            container_path: "/workspace".into(),
        }]);

        let opts = request_to_run_opts(&req, "inst-a");

        assert_eq!(opts.image.0, "ur-worker-claude:latest");
        assert_eq!(opts.name, "test-container");
        assert_eq!(opts.cpus, 2);
        assert_eq!(opts.memory, "4G");
        assert_eq!(opts.volumes.len(), 1);
        assert_eq!(opts.volumes[0].0, PathBuf::from("/tmp"));
        assert_eq!(opts.volumes[0].1, PathBuf::from("/workspace"));
        assert_eq!(opts.env_vars.len(), 1);
        assert_eq!(opts.env_vars[0], ("FOO".into(), "bar".into()));
        assert_eq!(opts.workdir, Some(PathBuf::from("/workspace")));
        assert_eq!(opts.network, Some("ur".into()));
        assert_eq!(opts.add_hosts.len(), 1);
        assert_eq!(
            opts.add_hosts[0],
            ("host.docker.internal".into(), "172.17.0.1".into())
        );
    }

    #[test]
    fn request_to_run_opts_empty_workdir() {
        let mut req = make_request(vec![]);
        req.workdir = String::new();
        let opts = request_to_run_opts(&req, "inst-a");
        assert_eq!(opts.workdir, None);
    }

    #[test]
    fn request_to_run_opts_empty_network() {
        let mut req = make_request(vec![]);
        req.network = String::new();
        let opts = request_to_run_opts(&req, "inst-a");
        assert_eq!(opts.network, None);
    }

    #[test]
    fn missing_volume_source_returns_failed_precondition() {
        let req = make_request(vec![Volume {
            host_path: "/nonexistent/path/xyz/abc".into(),
            container_path: "/workspace".into(),
        }]);

        let result = check_volume_sources(&req);
        assert!(result.is_err());
        let msg = result.unwrap_err();
        assert!(
            msg.contains("/nonexistent/path/xyz/abc"),
            "error should contain the missing path, got: {msg}"
        );
    }

    #[test]
    fn existing_volume_source_passes() {
        let req = make_request(vec![Volume {
            host_path: "/tmp".into(),
            container_path: "/workspace".into(),
        }]);

        assert!(check_volume_sources(&req).is_ok());
    }

    #[test]
    fn no_such_container_detection() {
        assert!(is_no_such_container("Error: No such container: abc123"));
        assert!(is_no_such_container(
            "docker stop failed: No such container xyz"
        ));
        assert!(!is_no_such_container("docker stop failed: timeout"));
        assert!(!is_no_such_container(""));
    }

    #[test]
    fn startup_wait_surfaces_container_exit_logs() {
        let mut states = VecDeque::from([
            StartupState::Starting,
            StartupState::Exited {
                logs: "startup hook 10-fail failed with status 7".into(),
            },
        ]);

        let error = wait_for_worker_startup(Duration::from_secs(1), Duration::ZERO, || {
            Ok(states.pop_front().unwrap())
        })
        .unwrap_err();

        assert!(error.contains("startup hook 10-fail"), "{error}");
    }
}
