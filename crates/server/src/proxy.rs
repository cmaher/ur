use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};
use bytes::Bytes;
use http::Method;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use tracing::{info, warn};

/// Manages an HTTP/HTTPS forward proxy for container network access control.
///
/// The proxy checks each request's target hostname against an allowlist.
/// Allowed requests are forwarded; blocked requests receive `403 Forbidden`.
///
/// HTTPS is handled via CONNECT tunneling (no TLS termination).
/// HTTP is handled by forwarding the request to the upstream server.
#[derive(Clone)]
pub struct ProxyManager {
    allowlist: Arc<RwLock<HashSet<String>>>,
}

impl ProxyManager {
    pub fn new(allowlist: Arc<RwLock<HashSet<String>>>) -> Self {
        Self { allowlist }
    }

    /// Bind to `bind_addr` and serve the forward proxy.
    ///
    /// Returns a `JoinHandle` for the accept loop. The proxy runs until the
    /// handle is aborted or the process exits.
    pub async fn serve(&self, bind_addr: SocketAddr) -> Result<JoinHandle<()>> {
        let listener = TcpListener::bind(bind_addr)
            .await
            .with_context(|| format!("failed to bind proxy to {bind_addr}"))?;
        info!(%bind_addr, "proxy listening");

        let manager = self.clone();
        let handle = tokio::spawn(async move {
            accept_loop(&listener, &manager).await;
        });

        Ok(handle)
    }

    async fn handle_request(
        &self,
        req: Request<Incoming>,
    ) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
        if req.method() == Method::CONNECT {
            self.handle_connect(req).await
        } else {
            self.handle_http(req).await
        }
    }

    /// Handle CONNECT requests for HTTPS tunneling.
    ///
    /// Extracts the hostname from the CONNECT target (host:port), checks the
    /// allowlist, and if permitted, responds with 200 and then upgrades the
    /// connection to bidirectionally tunnel raw TCP bytes.
    async fn handle_connect(
        &self,
        req: Request<Incoming>,
    ) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
        let Some(authority) = req.uri().authority().map(|a| a.to_string()) else {
            warn!(uri = %req.uri(), "CONNECT missing authority");
            return Ok(bad_request("missing authority in CONNECT request"));
        };

        let hostname = extract_host_from_authority(&authority);

        if !self.is_allowed(&hostname).await {
            warn!(hostname, "CONNECT blocked");
            return Ok(forbidden(&hostname));
        }

        let upstream_addr = authority.clone();

        // Spawn the tunnel task that runs after the upgrade completes.
        tokio::spawn(async move {
            run_connect_tunnel(req, upstream_addr).await;
        });

        Ok(Response::new(Full::default()))
    }

    /// Handle plain HTTP requests by forwarding them to the upstream server.
    ///
    /// Extracts the Host from the URI or the Host header, checks the allowlist,
    /// and if permitted, opens a TCP connection to the upstream, sends the
    /// request, and relays the response back.
    async fn handle_http(
        &self,
        req: Request<Incoming>,
    ) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
        let Some(hostname) = extract_http_host(&req) else {
            warn!(uri = %req.uri(), "HTTP request missing host");
            return Ok(bad_request("missing Host in HTTP request"));
        };

        if !self.is_allowed(&hostname).await {
            warn!(hostname, uri = %req.uri(), "HTTP blocked");
            return Ok(forbidden(&hostname));
        }

        let Some(upstream_addr) = resolve_http_upstream(&req) else {
            warn!(uri = %req.uri(), "HTTP request has no usable upstream address");
            return Ok(bad_request("cannot determine upstream address"));
        };

        let upstream = match TcpStream::connect(&upstream_addr).await {
            Ok(s) => s,
            Err(e) => {
                warn!(target_addr = upstream_addr, error = %e, "HTTP upstream connection failed");
                return Ok(bad_gateway(&format!("upstream connection failed: {e}")));
            }
        };

        info!(hostname, uri = %req.uri(), "HTTP forwarding");
        forward_http_request(req, upstream, &upstream_addr).await
    }

    async fn is_allowed(&self, hostname: &str) -> bool {
        let allowlist = self.allowlist.read().await;
        allowlist.contains(hostname)
    }
}

/// Accept loop: continuously accepts connections and spawns per-connection tasks.
async fn accept_loop(listener: &TcpListener, manager: &ProxyManager) {
    loop {
        let (stream, peer_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!(error = %e, "proxy accept error");
                continue;
            }
        };

        let manager = manager.clone();
        tokio::spawn(async move {
            serve_connection(stream, peer_addr, manager).await;
        });
    }
}

/// Serve a single HTTP/1.1 connection, dispatching requests to the proxy manager.
async fn serve_connection(stream: TcpStream, peer_addr: SocketAddr, manager: ProxyManager) {
    let io = TokioIo::new(stream);
    let service = service_fn(move |req| {
        let manager = manager.clone();
        async move { manager.handle_request(req).await }
    });

    let result = http1::Builder::new()
        .preserve_header_case(true)
        .title_case_headers(true)
        .serve_connection(io, service)
        .with_upgrades()
        .await;

    if let Err(e) = result
        && !is_benign_connection_error(&e)
    {
        warn!(%peer_addr, error = %e, "proxy connection error");
    }
}

/// Run the CONNECT tunnel after the HTTP upgrade completes.
async fn run_connect_tunnel(req: Request<Incoming>, upstream_addr: String) {
    let upgraded = match hyper::upgrade::on(req).await {
        Ok(u) => u,
        Err(e) => {
            warn!(error = %e, "CONNECT upgrade failed");
            return;
        }
    };

    let mut upgraded = TokioIo::new(upgraded);

    let mut upstream = match TcpStream::connect(&upstream_addr).await {
        Ok(s) => s,
        Err(e) => {
            warn!(target = upstream_addr, error = %e, "CONNECT upstream connection failed");
            return;
        }
    };

    info!(target = upstream_addr, "CONNECT tunnel established");

    match tokio::io::copy_bidirectional(&mut upgraded, &mut upstream).await {
        Ok((client_to_server, server_to_client)) => {
            info!(
                target = upstream_addr,
                client_to_server, server_to_client, "CONNECT tunnel closed"
            );
        }
        Err(e) => {
            info!(target = upstream_addr, error = %e, "CONNECT tunnel I/O error");
        }
    }
}

/// Forward an HTTP request to the upstream server and relay the response.
async fn forward_http_request(
    req: Request<Incoming>,
    upstream: TcpStream,
    upstream_addr: &str,
) -> std::result::Result<Response<Full<Bytes>>, hyper::Error> {
    let io = TokioIo::new(upstream);

    let (mut sender, conn) = match hyper::client::conn::http1::handshake(io).await {
        Ok(pair) => pair,
        Err(e) => {
            warn!(target_addr = upstream_addr, error = %e, "HTTP upstream handshake failed");
            return Ok(bad_gateway(&format!("upstream handshake failed: {e}")));
        }
    };

    // Drive the connection in the background
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            warn!(error = %e, "HTTP upstream connection error");
        }
    });

    let resp = match sender.send_request(req).await {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "HTTP upstream request failed");
            return Ok(bad_gateway(&format!("upstream request failed: {e}")));
        }
    };

    match resp.into_body().collect().await {
        Ok(body) => Ok(Response::new(Full::new(body.to_bytes()))),
        Err(e) => {
            warn!(error = %e, "HTTP upstream body read failed");
            Ok(bad_gateway(&format!("upstream body read failed: {e}")))
        }
    }
}

/// Extract the hostname from an authority string (`host:port` -> `host`).
fn extract_host_from_authority(authority: &str) -> String {
    // Handle IPv6 addresses like [::1]:443
    if let Some(bracket_end) = authority.find(']') {
        authority[..=bracket_end].to_string()
    } else if let Some(colon) = authority.rfind(':') {
        authority[..colon].to_string()
    } else {
        authority.to_string()
    }
}

/// Extract the hostname for an HTTP (non-CONNECT) request.
///
/// Tries the URI host first, then the Host header.
fn extract_http_host<B>(req: &Request<B>) -> Option<String> {
    // Try URI authority first (absolute-form)
    if let Some(host) = req.uri().host() {
        return Some(host.to_string());
    }

    // Fall back to Host header
    req.headers()
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(extract_host_from_authority)
}

/// Determine the upstream address for an HTTP request.
///
/// Returns `host:port` suitable for `TcpStream::connect`.
fn resolve_http_upstream<B>(req: &Request<B>) -> Option<String> {
    if let Some(authority) = req.uri().authority() {
        let host = authority.host();
        let port = authority.port_u16().unwrap_or(80);
        return Some(format!("{host}:{port}"));
    }

    // Fall back to Host header
    req.headers()
        .get(http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|h| {
            if h.contains(':') {
                h.to_string()
            } else {
                format!("{h}:80")
            }
        })
}

fn forbidden(hostname: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(403)
        .body(Full::new(Bytes::from(format!(
            "Forbidden: {hostname} is not in the proxy allowlist\n"
        ))))
        .expect("static response builder should not fail")
}

fn bad_request(reason: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(400)
        .body(Full::new(Bytes::from(format!("Bad Request: {reason}\n"))))
        .expect("static response builder should not fail")
}

fn bad_gateway(reason: &str) -> Response<Full<Bytes>> {
    Response::builder()
        .status(502)
        .body(Full::new(Bytes::from(format!("Bad Gateway: {reason}\n"))))
        .expect("static response builder should not fail")
}

/// Check if a hyper connection error is benign (client disconnected, etc.).
fn is_benign_connection_error(err: &hyper::Error) -> bool {
    if err.is_incomplete_message() {
        return true;
    }
    if err.is_canceled() {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_host_from_authority_with_port() {
        assert_eq!(
            extract_host_from_authority("example.com:443"),
            "example.com"
        );
    }

    #[test]
    fn extract_host_from_authority_without_port() {
        assert_eq!(extract_host_from_authority("example.com"), "example.com");
    }

    #[test]
    fn extract_host_from_authority_ipv6() {
        assert_eq!(extract_host_from_authority("[::1]:443"), "[::1]");
    }

    #[test]
    fn extract_host_from_authority_ipv4_with_port() {
        assert_eq!(
            extract_host_from_authority("192.168.1.1:8080"),
            "192.168.1.1"
        );
    }

    #[tokio::test]
    async fn allowlist_blocks_unknown_host() {
        let allowlist = Arc::new(RwLock::new(HashSet::from(
            ["api.anthropic.com".to_string()],
        )));
        let manager = ProxyManager::new(allowlist);
        assert!(!manager.is_allowed("evil.com").await);
    }

    #[tokio::test]
    async fn allowlist_allows_known_host() {
        let allowlist = Arc::new(RwLock::new(HashSet::from(
            ["api.anthropic.com".to_string()],
        )));
        let manager = ProxyManager::new(allowlist);
        assert!(manager.is_allowed("api.anthropic.com").await);
    }

    #[tokio::test]
    async fn allowlist_is_exact_match() {
        let allowlist = Arc::new(RwLock::new(HashSet::from(
            ["api.anthropic.com".to_string()],
        )));
        let manager = ProxyManager::new(allowlist);
        assert!(!manager.is_allowed("evil.api.anthropic.com").await);
        assert!(!manager.is_allowed("api.anthropic.com.evil.com").await);
    }

    #[tokio::test]
    async fn allowlist_empty_blocks_all() {
        let allowlist = Arc::new(RwLock::new(HashSet::new()));
        let manager = ProxyManager::new(allowlist);
        assert!(!manager.is_allowed("api.anthropic.com").await);
    }

    #[tokio::test]
    async fn allowlist_multiple_entries() {
        let allowlist = Arc::new(RwLock::new(HashSet::from([
            "api.anthropic.com".to_string(),
            "example.com".to_string(),
        ])));
        let manager = ProxyManager::new(allowlist);
        assert!(manager.is_allowed("api.anthropic.com").await);
        assert!(manager.is_allowed("example.com").await);
        assert!(!manager.is_allowed("other.com").await);
    }

    #[test]
    fn forbidden_response_has_403_status() {
        let resp = forbidden("evil.com");
        assert_eq!(resp.status(), 403);
    }

    #[test]
    fn bad_request_response_has_400_status() {
        let resp = bad_request("missing host");
        assert_eq!(resp.status(), 400);
    }

    #[test]
    fn bad_gateway_response_has_502_status() {
        let resp = bad_gateway("connection refused");
        assert_eq!(resp.status(), 502);
    }

    #[test]
    fn extract_http_host_from_uri() {
        let req = Request::builder()
            .uri("http://example.com/path")
            .body(())
            .unwrap();
        assert_eq!(extract_http_host(&req), Some("example.com".to_string()));
    }

    #[test]
    fn extract_http_host_from_header() {
        let req = Request::builder()
            .uri("/path")
            .header(http::header::HOST, "example.com:8080")
            .body(())
            .unwrap();
        assert_eq!(extract_http_host(&req), Some("example.com".to_string()));
    }

    #[test]
    fn extract_http_host_missing() {
        let req = Request::builder().uri("/path").body(()).unwrap();
        assert_eq!(extract_http_host(&req), None);
    }

    #[test]
    fn resolve_http_upstream_from_uri() {
        let req = Request::builder()
            .uri("http://example.com:8080/path")
            .body(())
            .unwrap();
        assert_eq!(
            resolve_http_upstream(&req),
            Some("example.com:8080".to_string())
        );
    }

    #[test]
    fn resolve_http_upstream_default_port() {
        let req = Request::builder()
            .uri("http://example.com/path")
            .body(())
            .unwrap();
        assert_eq!(
            resolve_http_upstream(&req),
            Some("example.com:80".to_string())
        );
    }

    #[test]
    fn resolve_http_upstream_from_host_header() {
        let req = Request::builder()
            .uri("/path")
            .header(http::header::HOST, "example.com:9090")
            .body(())
            .unwrap();
        assert_eq!(
            resolve_http_upstream(&req),
            Some("example.com:9090".to_string())
        );
    }

    #[test]
    fn resolve_http_upstream_host_header_default_port() {
        let req = Request::builder()
            .uri("/path")
            .header(http::header::HOST, "example.com")
            .body(())
            .unwrap();
        assert_eq!(
            resolve_http_upstream(&req),
            Some("example.com:80".to_string())
        );
    }
}
