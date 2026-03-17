use std::collections::HashMap;
use std::net::SocketAddr;

use tonic::transport::Server;
use ur_db::WorkerRepo;

use ur_rpc::proto::core::core_service_server::CoreServiceServer;

use crate::WorkerManager;
use crate::grpc::CoreServiceHandler;

/// Start the host gRPC server on a TCP socket.
///
/// Serves the host CLI (`ur`). Registers Core, RAG, and Ticket services
/// directly — no auth interceptor.
pub async fn serve_grpc(
    addr: SocketAddr,
    handler: CoreServiceHandler,
    #[cfg(feature = "rag")] rag_handler: crate::rag::RagServiceHandler,
    #[cfg(feature = "ticket")] ticket_handler: crate::grpc_ticket::TicketServiceHandler,
) -> anyhow::Result<()> {
    tracing::info!(addr = %addr, "host gRPC server listening");

    let mut router = Server::builder().add_service(CoreServiceServer::new(handler));

    #[cfg(feature = "rag")]
    {
        use ur_rpc::proto::rag::rag_service_server::RagServiceServer;
        router = router.add_service(RagServiceServer::new(rag_handler));
    }

    #[cfg(feature = "ticket")]
    {
        use ur_rpc::proto::ticket::ticket_service_server::TicketServiceServer;
        router = router.add_service(TicketServiceServer::new(ticket_handler));
    }

    router.serve(addr).await?;

    Ok(())
}

/// Start the shared worker gRPC server on a TCP socket.
///
/// Serves all container workers. Registers HostExec, RAG, and Ticket services,
/// all wrapped with the worker auth interceptor that validates `ur-worker-id` and
/// `ur-worker-secret` metadata headers via `WorkerRepo`.
#[allow(unused_variables, clippy::too_many_arguments)]
pub async fn serve_worker_grpc(
    addr: SocketAddr,
    worker_manager: WorkerManager,
    worker_repo: WorkerRepo,
    projects: HashMap<String, ur_config::ProjectConfig>,
    #[cfg(feature = "hostexec")] hostexec_config: crate::hostexec::HostExecConfigManager,
    #[cfg(feature = "hostexec")] builderd_addr: String,
    #[cfg(feature = "hostexec")] host_workspace: std::path::PathBuf,
    #[cfg(feature = "rag")] rag_handler: crate::rag::RagServiceHandler,
    #[cfg(feature = "ticket")] ticket_handler: crate::grpc_ticket::TicketServiceHandler,
) -> anyhow::Result<()> {
    tracing::info!(addr = %addr, "worker gRPC server listening");

    let worker_core_handler = crate::grpc::WorkerCoreServiceHandler {
        worker_repo: worker_repo.clone(),
    };
    let interceptor = crate::auth::worker_auth_interceptor(worker_repo);

    // Build the Routes collection, wrapping each service with the auth interceptor.
    let mut routes = tonic::service::Routes::builder();

    // Register CoreService (ping + agent status updates) for worker containers.
    routes.add_service(CoreServiceServer::with_interceptor(
        worker_core_handler,
        interceptor.clone(),
    ));

    #[cfg(feature = "hostexec")]
    {
        use ur_rpc::proto::hostexec::host_exec_service_server::HostExecServiceServer;

        let hostexec_handler = crate::grpc_hostexec::HostExecServiceHandler {
            config: hostexec_config,
            lua: crate::hostexec::LuaTransformManager::new(),
            worker_manager,
            projects,
            builderd_addr,
            host_workspace,
        };

        routes.add_service(HostExecServiceServer::with_interceptor(
            hostexec_handler,
            interceptor.clone(),
        ));
    }

    #[cfg(feature = "rag")]
    {
        use ur_rpc::proto::rag::rag_service_server::RagServiceServer;
        routes.add_service(RagServiceServer::with_interceptor(
            rag_handler,
            interceptor.clone(),
        ));
    }

    #[cfg(feature = "ticket")]
    {
        use ur_rpc::proto::ticket::ticket_service_server::TicketServiceServer;
        routes.add_service(TicketServiceServer::with_interceptor(
            ticket_handler,
            interceptor.clone(),
        ));
    }

    Server::builder()
        .add_routes(routes.routes())
        .serve(addr)
        .await?;

    Ok(())
}
