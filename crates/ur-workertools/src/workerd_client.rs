use tonic::transport::{Channel, Endpoint};

use ur_rpc::proto::workerd::worker_daemon_service_client::WorkerDaemonServiceClient;

/// Resolve the workerd gRPC URL from the value of `UR_WORKERD_ADDR`.
///
/// Unset or blank falls back to the in-container daemon on
/// `127.0.0.1:WORKERD_GRPC_PORT`. A bare `host:port` gets an `http://` scheme.
fn resolve_workerd_addr(env_value: Option<String>) -> String {
    match env_value.map(|v| v.trim().to_string()) {
        Some(addr) if !addr.is_empty() => {
            if addr.contains("://") {
                addr
            } else {
                format!("http://{addr}")
            }
        }
        _ => format!("http://127.0.0.1:{}", ur_config::WORKERD_GRPC_PORT),
    }
}

/// The workerd address this process should dial, honoring `UR_WORKERD_ADDR`.
pub(crate) fn workerd_addr() -> String {
    resolve_workerd_addr(std::env::var(ur_config::UR_WORKERD_ADDR_ENV).ok())
}

/// Connect to workerd at [`workerd_addr`]. `context` prefixes the error log
/// line (e.g. `notify-idle`, `status`). Returns the process exit code on failure.
pub(crate) async fn connect_workerd(
    context: &str,
) -> Result<WorkerDaemonServiceClient<Channel>, i32> {
    let addr = workerd_addr();
    let endpoint = match Endpoint::try_from(addr.clone()) {
        Ok(ep) => ep,
        Err(e) => {
            eprintln!(
                "[ur-workertools] {context}: invalid {} '{addr}': {e}",
                ur_config::UR_WORKERD_ADDR_ENV
            );
            return Err(1);
        }
    };
    match endpoint.connect().await {
        Ok(channel) => Ok(WorkerDaemonServiceClient::new(channel)),
        Err(e) => {
            eprintln!("[ur-workertools] {context}: failed to connect to workerd at {addr}: {e}");
            Err(1)
        }
    }
}

/// Wrap a WorkerDaemonService message in a request carrying the worker auth
/// headers. Container workerd ignores them; a shared host daemon routes on
/// `ur-worker-id`.
pub(crate) fn workerd_request<T>(message: T) -> tonic::Request<T> {
    let mut request = tonic::Request::new(message);
    crate::inject_auth(&mut request);
    request
}

#[cfg(test)]
mod tests {
    use super::resolve_workerd_addr;

    #[test]
    fn unset_or_blank_defaults_to_container_localhost() {
        let expected = format!("http://127.0.0.1:{}", ur_config::WORKERD_GRPC_PORT);
        assert_eq!(resolve_workerd_addr(None), expected);
        assert_eq!(resolve_workerd_addr(Some(String::new())), expected);
        assert_eq!(resolve_workerd_addr(Some("  ".to_string())), expected);
    }

    #[test]
    fn url_is_used_verbatim() {
        assert_eq!(
            resolve_workerd_addr(Some("http://127.0.0.1:42070".to_string())),
            "http://127.0.0.1:42070"
        );
    }

    #[test]
    fn bare_host_port_gets_http_scheme() {
        assert_eq!(
            resolve_workerd_addr(Some("localhost:42070".to_string())),
            "http://localhost:42070"
        );
    }
}
