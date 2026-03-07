use std::path::Path;

use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

use ur_rpc::proto::core::core_service_server::CoreServiceServer;

use crate::grpc::CoreServiceHandler;

/// Start the tonic gRPC server on a Unix domain socket.
///
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
