use std::net::SocketAddr;

use tonic::service::Routes;
use tonic::transport::Server;

use ur_rpc::proto::core::core_service_server::CoreServiceServer;

use crate::grpc::CoreServiceHandler;

/// Build a Routes collection with all enabled services.
fn build_agent_routes(core_handler: CoreServiceHandler, process_id: &str) -> Routes {
    let mut builder = Routes::builder();
    builder.add_service(CoreServiceServer::new(core_handler.clone()));

    #[cfg(feature = "git")]
    {
        use ur_rpc::proto::git::git_service_server::GitServiceServer;
        builder.add_service(GitServiceServer::new(crate::grpc_git::GitServiceHandler {
            repo_registry: core_handler.repo_registry.clone(),
            process_id: process_id.to_owned(),
        }));
    }

    #[cfg(feature = "gh")]
    {
        use ur_rpc::proto::gh::gh_service_server::GhServiceServer;
        builder.add_service(GhServiceServer::new(crate::grpc_gh::GhServiceHandler {
            repo_registry: core_handler.repo_registry.clone(),
            process_id: process_id.to_owned(),
        }));
    }

    builder.routes()
}

/// Start the main tonic gRPC server on a TCP socket.
///
/// Used for the main host CLI path (ur -> urd).
pub async fn serve_grpc(addr: SocketAddr, handler: CoreServiceHandler) -> anyhow::Result<()> {
    tracing::info!("gRPC server listening on {addr}");

    let routes = build_agent_routes(handler, "");

    Server::builder()
        .add_routes(routes)
        .serve(addr)
        .await?;

    Ok(())
}

/// Start a per-agent gRPC server on TCP, bound to the given host IP with an
/// OS-assigned port.
///
/// Binds the listener, spawns the server task, and returns the assigned port
/// plus a `JoinHandle` the caller can abort to stop the server.
///
/// `bind_host` should be the host gateway IP (e.g. 192.168.64.x for Apple,
/// 172.17.0.x for Docker) so the server is reachable from containers but
/// not exposed on the local network.
pub async fn serve_agent_grpc(
    bind_host: &str,
    core_handler: CoreServiceHandler,
    process_id: &str,
) -> anyhow::Result<(u16, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(format!("{bind_host}:0")).await?;
    let addr = listener.local_addr()?;
    let port = addr.port();

    tracing::info!("per-agent gRPC server listening on {addr}");

    let routes = build_agent_routes(core_handler, process_id);
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let handle = tokio::spawn(async move {
        let result = Server::builder()
            .add_routes(routes)
            .serve_with_incoming(incoming)
            .await;

        if let Err(e) = result {
            tracing::warn!("per-agent gRPC server error: {e}");
        }
    });

    Ok((port, handle))
}
