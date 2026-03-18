use anyhow::{Result, bail};
use tonic::transport::{Channel, Endpoint};
use tracing::{debug, error, info, instrument};

/// Connect to the ur-server at the given port, returning a raw tonic Channel.
///
/// Fails with a user-facing error if the server is unreachable.
#[instrument]
pub async fn connect(port: u16) -> Result<Channel> {
    match try_connect(port).await {
        Some(channel) => Ok(channel),
        None => {
            error!(port, "server is not running");
            bail!("server is not running \u{2014} run 'ur server start' first")
        }
    }
}

/// Attempt to connect to the ur-server at the given port.
///
/// Returns `None` if the server is unreachable (used for graceful-stop probing).
#[instrument]
pub async fn try_connect(port: u16) -> Option<Channel> {
    let addr = format!("http://127.0.0.1:{port}");
    debug!(addr, "attempting gRPC connection");
    let channel = Endpoint::try_from(addr.clone())
        .ok()?
        .connect()
        .await
        .ok()?;
    info!(addr, "gRPC connection established");
    Some(channel)
}
