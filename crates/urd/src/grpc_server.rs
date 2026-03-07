use std::path::Path;

use tokio::net::UnixListener;
use tokio_stream::wrappers::UnixListenerStream;
use tonic::transport::Server;

use ur_rpc::proto::core::core_service_server::CoreServiceServer;

use crate::grpc::CoreServiceHandler;

/// Start the tonic gRPC server on a Unix domain socket.
pub async fn serve_grpc(socket_path: &Path, handler: CoreServiceHandler) -> anyhow::Result<()> {
    let _ = tokio::fs::remove_file(socket_path).await;

    let listener = UnixListener::bind(socket_path)?;
    let stream = UnixListenerStream::new(listener);

    tracing::info!("gRPC server listening on {}", socket_path.display());

    Server::builder()
        .add_service(CoreServiceServer::new(handler))
        .serve_with_incoming(stream)
        .await?;

    Ok(())
}
