use std::net::SocketAddr;

use ticket_db::TicketRepo;
use tonic::transport::Server;
use workflow_db::{WorkerRepo, WorkflowRepo};

use ur_rpc::proto::core::core_service_server::CoreServiceServer;
use ur_rpc::proto::hostexec::host_exec_service_server::HostExecServiceServer;
use ur_rpc::proto::remote_repo::remote_repo_service_server::RemoteRepoServiceServer;
use ur_rpc::proto::ticket::ticket_service_server::TicketServiceServer;

use crate::grpc::{CoreServiceHandler, LaunchManager};
use crate::{ProjectRegistry, WorkerManager};

/// Start the host gRPC server on a TCP socket.
///
/// Serves the host CLI (`ur`). Registers Core and Ticket services
/// directly — no auth interceptor.
pub async fn serve_grpc(
    addr: SocketAddr,
    handler: CoreServiceHandler,
    ticket_handler: crate::grpc_ticket::TicketServiceHandler,
) -> anyhow::Result<()> {
    tracing::info!(addr = %addr, "host gRPC server listening");

    let router = Server::builder()
        .add_service(CoreServiceServer::new(handler))
        .add_service(TicketServiceServer::new(ticket_handler));

    router.serve(addr).await?;

    Ok(())
}

/// Start the shared worker gRPC server on a TCP socket.
///
/// Serves all container workers. Registers HostExec and Ticket services,
/// all wrapped with the worker auth interceptor that validates `ur-worker-id` and
/// `ur-worker-secret` metadata headers via `WorkerRepo`.
#[allow(clippy::too_many_arguments)]
pub async fn serve_worker_grpc(
    addr: SocketAddr,
    worker_manager: WorkerManager,
    worker_repo: WorkerRepo,
    ticket_repo: TicketRepo,
    workflow_repo: WorkflowRepo,
    worker_prefix: String,
    project_registry: ProjectRegistry,
    builderd_addr: String,
    host_workspace: std::path::PathBuf,
    git_branch_prefix: String,
    ticket_handler: crate::grpc_ticket::TicketServiceHandler,
    remote_repo_handler: crate::grpc_remote_repo::RemoteRepoServiceHandler,
    transition_tx: tokio::sync::mpsc::Sender<crate::workflow::TransitionRequest>,
    launch_manager: LaunchManager,
) -> anyhow::Result<()> {
    tracing::info!(addr = %addr, "worker gRPC server listening");

    let worker_core_handler = crate::grpc::WorkerCoreServiceHandler {
        worker_repo: worker_repo.clone(),
        ticket_repo,
        workflow_repo,
        worker_prefix,
        transition_tx,
        launch_manager,
    };
    let interceptor = crate::auth::worker_auth_interceptor(worker_repo);

    // Build the Routes collection, wrapping each service with the auth interceptor.
    let mut routes = tonic::service::Routes::builder();

    // Register CoreService (ping + agent status updates) for worker containers.
    routes.add_service(CoreServiceServer::with_interceptor(
        worker_core_handler,
        interceptor.clone(),
    ));

    {
        let hostexec_retry_channel =
            ur_rpc::retry::RetryChannel::new(&builderd_addr, ur_rpc::retry::RetryConfig::default())
                .expect("failed to create builderd retry channel for hostexec");
        let hostexec_builderd_client =
            ur_rpc::proto::builder::BuilderdClient::new(hostexec_retry_channel.channel().clone());
        let hostexec_handler = crate::grpc_hostexec::HostExecServiceHandler {
            project_registry,
            lua: crate::hostexec::LuaTransformManager::new(),
            worker_manager,
            builderd_client: hostexec_builderd_client,
            host_workspace,
            git_branch_prefix,
        };

        routes.add_service(HostExecServiceServer::with_interceptor(
            hostexec_handler,
            interceptor.clone(),
        ));
    }

    routes.add_service(TicketServiceServer::with_interceptor(
        ticket_handler,
        interceptor.clone(),
    ));

    routes.add_service(RemoteRepoServiceServer::with_interceptor(
        remote_repo_handler,
        interceptor.clone(),
    ));

    Server::builder()
        .add_routes(routes.routes())
        .serve(addr)
        .await?;

    Ok(())
}
