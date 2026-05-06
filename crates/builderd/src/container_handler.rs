use std::path::PathBuf;

use tonic::{Request, Response, Status};
use tracing::{error, info, warn};

use container::{
    ContainerId, ContainerRuntime, DockerRuntime, ExecOpts, ImageId, PortMap, RunOpts,
    runtime_from_env,
};
use ur_rpc::proto::builder_container::builder_container_service_server::BuilderContainerService;
use ur_rpc::proto::builder_container::{
    ExecContainerRequest, ExecContainerResponse, InspectNetworkRequest, InspectNetworkResponse,
    LaunchWorkerRequest, LaunchWorkerResponse, StopWorkerRequest, StopWorkerResponse,
};

/// Handles BuilderContainerService RPCs: launch, stop, exec, and network inspect
/// for worker containers. Uses DockerRuntime from the container crate.
#[derive(Clone)]
pub struct BuilderContainerHandler {
    runtime: DockerRuntime,
}

impl BuilderContainerHandler {
    pub fn new() -> Self {
        Self {
            runtime: runtime_from_env(),
        }
    }
}

/// Check whether a docker "No such container" message appears in the given text.
fn is_no_such_container(text: &str) -> bool {
    text.contains("No such container")
}

/// Convert a `LaunchWorkerRequest` into `RunOpts` for the container runtime.
fn request_to_run_opts(req: &LaunchWorkerRequest) -> RunOpts {
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

#[tonic::async_trait]
impl BuilderContainerService for BuilderContainerHandler {
    async fn launch_worker(
        &self,
        req: Request<LaunchWorkerRequest>,
    ) -> Result<Response<LaunchWorkerResponse>, Status> {
        let req = req.into_inner();

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

        let opts = request_to_run_opts(&req);
        let container_id = self.runtime.run(&opts).map_err(|e| {
            error!(error = %e, name = %req.name, "docker run failed");
            Status::internal(format!("docker run failed: {e}"))
        })?;

        info!(container_id = %container_id.0, name = %req.name, "container launched");
        Ok(Response::new(LaunchWorkerResponse {
            container_id: container_id.0,
        }))
    }

    async fn stop_worker(
        &self,
        req: Request<StopWorkerRequest>,
    ) -> Result<Response<StopWorkerResponse>, Status> {
        let req = req.into_inner();
        let id = ContainerId(req.container_id.clone());

        info!(container_id = %req.container_id, "StopWorker request received");

        // Attempt stop; tolerate "No such container" as NotFound.
        let stop_result = self.runtime.stop(&id);
        match stop_result {
            Ok(()) => {}
            Err(e) => {
                let msg = e.to_string();
                if is_no_such_container(&msg) {
                    info!(container_id = %req.container_id, "container already gone (stop)");
                    return Err(Status::not_found(format!(
                        "container not found: {}",
                        req.container_id
                    )));
                }
                error!(container_id = %req.container_id, error = %msg, "docker stop failed");
                return Err(Status::internal(format!("docker stop failed: {msg}")));
            }
        }

        // Attempt rm; tolerate "No such container" as NotFound.
        let rm_result = self.runtime.rm(&id);
        match rm_result {
            Ok(()) => {}
            Err(e) => {
                let msg = e.to_string();
                if is_no_such_container(&msg) {
                    info!(container_id = %req.container_id, "container already gone (rm)");
                    return Err(Status::not_found(format!(
                        "container not found: {}",
                        req.container_id
                    )));
                }
                error!(container_id = %req.container_id, error = %msg, "docker rm failed");
                return Err(Status::internal(format!("docker rm failed: {msg}")));
            }
        }

        info!(container_id = %req.container_id, "container stopped and removed");
        Ok(Response::new(StopWorkerResponse {}))
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
    use ur_rpc::proto::builder_container::{AddHost, EnvVar, Volume};

    fn make_request(volumes: Vec<Volume>) -> LaunchWorkerRequest {
        LaunchWorkerRequest {
            image: "ur-worker:latest".into(),
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
        }
    }

    #[test]
    fn request_to_run_opts_conversion() {
        let req = make_request(vec![Volume {
            host_path: "/tmp".into(),
            container_path: "/workspace".into(),
        }]);

        let opts = request_to_run_opts(&req);

        assert_eq!(opts.image.0, "ur-worker:latest");
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
        let opts = request_to_run_opts(&req);
        assert_eq!(opts.workdir, None);
    }

    #[test]
    fn request_to_run_opts_empty_network() {
        let mut req = make_request(vec![]);
        req.network = String::new();
        let opts = request_to_run_opts(&req);
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
}
