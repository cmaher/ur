use anyhow::{Context, Result};
use tonic::metadata::{Ascii, MetadataValue};
use tonic::service::Interceptor;
use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;
use tracing::{debug, instrument};

use crate::retry::{RetryChannel, RetryConfig};

/// A channel to ur-server's host listener that presents the host token on
/// every request. Every host-side client of `server_port` uses this type.
pub type HostChannel = InterceptedService<Channel, HostTokenInterceptor>;

/// Client interceptor inserting the host token header
/// ([`ur_config::HOST_TOKEN_HEADER`]) into each request's metadata.
#[derive(Clone)]
pub struct HostTokenInterceptor {
    token: MetadataValue<Ascii>,
}

impl HostTokenInterceptor {
    /// Build an interceptor for `token`, which must be a valid host token.
    pub fn new(token: &str) -> Result<Self> {
        ur_config::validate_host_token(token)?;
        let token = token
            .parse()
            .context("host token is not a valid gRPC metadata value")?;
        Ok(Self { token })
    }
}

impl Interceptor for HostTokenInterceptor {
    fn call(&mut self, mut req: tonic::Request<()>) -> Result<tonic::Request<()>, tonic::Status> {
        req.metadata_mut()
            .insert(ur_config::HOST_TOKEN_HEADER, self.token.clone());
        Ok(req)
    }
}

/// Connect to the ur-server host listener at the given port.
///
/// Reads the host token from `$UR_CONFIG/host-token` (see
/// [`ur_config::read_host_token`]); a missing or unreadable file is an error
/// naming the file. The channel is lazy, so this does not verify the server
/// is running — the first RPC does.
#[instrument]
pub async fn connect(port: u16) -> Result<HostChannel> {
    let config_dir = ur_config::resolve_config_dir()?;
    let token = ur_config::read_host_token(&config_dir)?;
    connect_with_token(port, &token)
}

/// Create a lazy retry channel to `127.0.0.1:<port>` presenting `token`.
pub fn connect_with_token(port: u16, token: &str) -> Result<HostChannel> {
    let addr = format!("http://127.0.0.1:{port}");
    debug!(addr, "creating lazy host gRPC channel");
    let retry_channel =
        RetryChannel::new(&addr, RetryConfig::default()).context("invalid server address")?;
    Ok(InterceptedService::new(
        retry_channel.channel().clone(),
        HostTokenInterceptor::new(token)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interceptor_inserts_token_header() {
        let token = "t".repeat(40);
        let mut interceptor = HostTokenInterceptor::new(&token).unwrap();
        let req = interceptor.call(tonic::Request::new(())).unwrap();
        let sent = req.metadata().get(ur_config::HOST_TOKEN_HEADER).unwrap();
        assert_eq!(sent.to_str().unwrap(), token);
    }

    #[test]
    fn interceptor_rejects_invalid_token() {
        assert!(HostTokenInterceptor::new("short").is_err());
    }
}
