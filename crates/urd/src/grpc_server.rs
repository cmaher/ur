use std::path::Path;

use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

use ur_rpc::proto::core::core_service_server::CoreServiceServer;

use crate::grpc::CoreServiceHandler;

/// Start the tonic gRPC server on a Unix domain socket.
///
/// Used for the main host CLI path (ur -> urd). Per-agent servers use TCP instead.
/// When the `git` feature is enabled, the `GitService` is also registered.
pub async fn serve_grpc(socket_path: &Path, handler: CoreServiceHandler) -> anyhow::Result<()> {
    let _ = tokio::fs::remove_file(socket_path).await;

    let listener = UnixListener::bind(socket_path)?;
    let stream = UnixListenerStream::new(listener);

    tracing::info!("gRPC server listening on {}", socket_path.display());

    let mut builder = Server::builder();

    // Always register the core service
    let router = builder.add_service(CoreServiceServer::new(handler.clone()));

    // Conditionally register the git service
    #[cfg(feature = "git")]
    let router = {
        use ur_rpc::proto::git::git_service_server::GitServiceServer;
        let git_handler = crate::grpc_git::GitServiceHandler {
            repo_registry: handler.repo_registry.clone(),
            process_id: String::new(),
        };
        router.add_service(GitServiceServer::new(git_handler))
    };

    router.serve_with_incoming(stream).await?;

    Ok(())
}

/// Start a per-agent gRPC server on TCP (127.0.0.1:0 for OS-assigned port).
///
/// Binds the listener, spawns the server task, and returns the assigned port
/// plus a `JoinHandle` the caller can abort to stop the server. The host-side
/// port is dynamically assigned; the container runtime maps it to the fixed
/// `agent_grpc_port` inside the container.
#[cfg(feature = "git")]
pub async fn serve_agent_grpc(
    core_handler: CoreServiceHandler,
    git_handler: crate::grpc_git::GitServiceHandler,
) -> anyhow::Result<(u16, tokio::task::JoinHandle<()>)> {
    use ur_rpc::proto::git::git_service_server::GitServiceServer;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let port = addr.port();

    tracing::info!("per-agent gRPC server listening on {addr}");

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let handle = tokio::spawn(async move {
        let result = Server::builder()
            .add_service(CoreServiceServer::new(core_handler))
            .add_service(GitServiceServer::new(git_handler))
            .serve_with_incoming(incoming)
            .await;

        if let Err(e) = result {
            tracing::warn!("per-agent gRPC server error: {e}");
        }
    });

    Ok((port, handle))
}

/// Start a per-agent gRPC server on TCP without the git service.
///
/// Fallback when the `git` feature is not enabled.
#[cfg(not(feature = "git"))]
pub async fn serve_agent_grpc(
    core_handler: CoreServiceHandler,
) -> anyhow::Result<(u16, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let port = addr.port();

    tracing::info!("per-agent gRPC server listening on {addr}");

    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);

    let handle = tokio::spawn(async move {
        let result = Server::builder()
            .add_service(CoreServiceServer::new(core_handler))
            .serve_with_incoming(incoming)
            .await;

        if let Err(e) = result {
            tracing::warn!("per-agent gRPC server error: {e}");
        }
    });

    Ok((port, handle))
}
