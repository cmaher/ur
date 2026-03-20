use anyhow::{Result, bail};
use tonic::transport::Channel;
use tracing::{debug, error, info, instrument};
use ur_rpc::retry::{RetryChannel, RetryConfig};

/// Connect to the ur-server at the given port, returning a raw tonic Channel.
///
/// Uses a lazy retry channel so the connection is established on first use,
/// with automatic retries on transient failures.
#[instrument]
pub async fn connect(port: u16) -> Result<Channel> {
    match try_connect(port) {
        Some(channel) => Ok(channel),
        None => {
            error!(port, "server is not running");
            bail!("server is not running \u{2014} run 'ur server start' first")
        }
    }
}

/// Attempt to create a lazy retry channel to the ur-server at the given port.
///
/// Returns `None` if the address is invalid. The channel connects lazily on
/// first RPC call, so this does not verify the server is running.
#[instrument]
pub fn try_connect(port: u16) -> Option<Channel> {
    let addr = format!("http://127.0.0.1:{port}");
    debug!(addr, "creating lazy gRPC retry channel");
    let retry_channel = RetryChannel::new(&addr, RetryConfig::default()).ok()?;
    info!(addr, "gRPC retry channel created");
    Some(retry_channel.channel().clone())
}
