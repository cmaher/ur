use std::net::SocketAddr;

use tonic::service::Routes;
use tonic::transport::Server;

use ur_rpc::proto::core::core_service_server::CoreServiceServer;

use crate::grpc::CoreServiceHandler;

/// Build a Routes collection with all enabled services.
fn build_agent_routes(core_handler: CoreServiceHandler, process_id: &str) -> Routes {
    let mut builder = Routes::builder();
    builder.add_service(CoreServiceServer::new(core_handler.clone()));

    #[cfg(feature = "hostexec")]
    {
        use ur_rpc::proto::hostexec::host_exec_service_server::HostExecServiceServer;
        builder.add_service(HostExecServiceServer::new(
            crate::grpc_hostexec::HostExecServiceHandler {
                config: core_handler.hostexec_config.clone(),
                lua: crate::hostexec::LuaTransformManager::new(),
                repo_registry: core_handler.repo_registry.clone(),
                process_id: process_id.to_owned(),
                hostd_addr: core_handler.hostd_addr.clone(),
            },
        ));
    }

    builder.routes()
}

/// Start the main tonic gRPC server on a TCP socket.
///
/// Used for the main host CLI path (ur -> server).
pub async fn serve_grpc(addr: SocketAddr, handler: CoreServiceHandler) -> anyhow::Result<()> {
    tracing::info!(addr = %addr, "main gRPC server listening");

    let routes = build_agent_routes(handler, "");

    Server::builder().add_routes(routes).serve(addr).await?;

    Ok(())
}

/// Start a per-agent gRPC server on TCP, bound to the given host address with
/// an OS-assigned port.
///
/// Binds the listener, spawns the server task, and returns the assigned port
/// plus a `JoinHandle` the caller can abort to stop the server.
///
/// `bind_host` is typically `0.0.0.0` — containers reach the server via Docker
/// internal DNS (the server hostname on the shared Docker network).
pub async fn serve_agent_grpc(
    bind_host: &str,
    core_handler: CoreServiceHandler,
    process_id: &str,
) -> anyhow::Result<(u16, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(format!("{bind_host}:0")).await?;
    let addr = listener.local_addr()?;
    let port = addr.port();

    tracing::info!(addr = %addr, port, process_id, "per-agent gRPC server listening");

    let routes = build_agent_routes(core_handler, process_id);
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let handle = tokio::spawn(async move {
        let result = Server::builder()
            .add_routes(routes)
            .serve_with_incoming(incoming)
            .await;

        if let Err(e) = result {
            tracing::warn!(error = %e, "per-agent gRPC server error");
        }
    });

    Ok((port, handle))
}
