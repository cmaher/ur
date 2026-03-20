use std::future;
use std::pin::Pin;
use std::time::Duration;

use tonic::transport::Channel;
use tower::retry::Policy;

/// Default maximum number of retry attempts.
pub const DEFAULT_MAX_RETRIES: u32 = 3;

/// Default base backoff in milliseconds (exponential: 200, 400, 800, ...).
pub const DEFAULT_BASE_BACKOFF_MS: u64 = 200;

/// Configuration for gRPC retry behavior.
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (not counting the initial request).
    pub max_retries: u32,
    /// Base backoff duration in milliseconds. Each retry doubles this.
    pub base_backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: DEFAULT_MAX_RETRIES,
            base_backoff_ms: DEFAULT_BASE_BACKOFF_MS,
        }
    }
}

/// Retry policy for gRPC requests. Retries on transient status codes
/// (Unavailable, Internal) with exponential backoff. Does NOT retry on
/// permanent errors (InvalidArgument, NotFound, PermissionDenied, etc.).
#[derive(Debug, Clone)]
pub struct GrpcRetryPolicy {
    max_retries: u32,
    base_backoff: Duration,
    attempts_remaining: u32,
}

impl GrpcRetryPolicy {
    fn new(config: &RetryConfig) -> Self {
        Self {
            max_retries: config.max_retries,
            base_backoff: Duration::from_millis(config.base_backoff_ms),
            attempts_remaining: config.max_retries,
        }
    }
}

/// Returns true if the given gRPC status code should be retried.
pub fn is_retryable(code: tonic::Code) -> bool {
    matches!(
        code,
        tonic::Code::Unavailable | tonic::Code::Internal | tonic::Code::Unknown
    )
}

/// Returns true if a tonic transport error message suggests a connection
/// failure that is worth retrying (connection refused, reset, etc.).
fn is_connection_error(status: &tonic::Status) -> bool {
    let msg = status.message().to_lowercase();
    msg.contains("connection refused")
        || msg.contains("connection reset")
        || msg.contains("broken pipe")
        || msg.contains("transport error")
}

impl<Req: Clone, Res> Policy<Req, Res, tonic::Status> for GrpcRetryPolicy {
    type Future = Pin<Box<dyn future::Future<Output = ()> + Send>>;

    fn retry(
        &mut self,
        _req: &mut Req,
        result: &mut Result<Res, tonic::Status>,
    ) -> Option<Self::Future> {
        match result {
            Ok(_) => None,
            Err(status) => {
                if self.attempts_remaining == 0 {
                    return None;
                }
                if !is_retryable(status.code()) && !is_connection_error(status) {
                    return None;
                }

                self.attempts_remaining -= 1;
                let attempt = self.max_retries - self.attempts_remaining;
                let backoff = self.base_backoff * 2u32.saturating_pow(attempt - 1);

                tracing::warn!(
                    code = %status.code(),
                    attempt,
                    max = self.max_retries,
                    backoff_ms = backoff.as_millis() as u64,
                    "retrying gRPC request after transient failure"
                );

                Some(Box::pin(async move {
                    tokio::time::sleep(backoff).await;
                }))
            }
        }
    }

    fn clone_request(&mut self, req: &Req) -> Option<Req> {
        Some(req.clone())
    }
}

/// A gRPC channel with automatic retry on transient failures.
///
/// Wraps `tonic::transport::Channel` with a tower retry layer using
/// `connect_lazy` so the channel reconnects on demand rather than
/// holding a persistent HTTP/2 stream.
#[derive(Debug, Clone)]
pub struct RetryChannel {
    inner: Channel,
    config: RetryConfig,
}

impl RetryChannel {
    /// Create a new retry-capable channel that connects lazily to the given address.
    ///
    /// The address should be a full URI, e.g. `"http://localhost:42070"`.
    pub fn new(addr: &str, config: RetryConfig) -> Result<Self, tonic::transport::Error> {
        let endpoint = tonic::transport::Endpoint::try_from(addr.to_string())?;
        let channel = endpoint.connect_lazy();
        Ok(Self {
            inner: channel,
            config,
        })
    }

    /// Returns the underlying tonic Channel (without retry).
    pub fn channel(&self) -> &Channel {
        &self.inner
    }

    /// Returns a tower service that wraps the channel with the retry policy.
    pub fn service(&self) -> tower::retry::Retry<GrpcRetryPolicy, Channel> {
        let policy = GrpcRetryPolicy::new(&self.config);
        tower::retry::Retry::new(policy, self.inner.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_values() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, DEFAULT_MAX_RETRIES);
        assert_eq!(config.base_backoff_ms, DEFAULT_BASE_BACKOFF_MS);
    }

    #[test]
    fn retryable_status_codes() {
        assert!(is_retryable(tonic::Code::Unavailable));
        assert!(is_retryable(tonic::Code::Internal));
        assert!(is_retryable(tonic::Code::Unknown));
    }

    #[test]
    fn non_retryable_status_codes() {
        assert!(!is_retryable(tonic::Code::InvalidArgument));
        assert!(!is_retryable(tonic::Code::NotFound));
        assert!(!is_retryable(tonic::Code::PermissionDenied));
        assert!(!is_retryable(tonic::Code::AlreadyExists));
        assert!(!is_retryable(tonic::Code::Unauthenticated));
        assert!(!is_retryable(tonic::Code::Ok));
        assert!(!is_retryable(tonic::Code::Cancelled));
        assert!(!is_retryable(tonic::Code::DeadlineExceeded));
        assert!(!is_retryable(tonic::Code::FailedPrecondition));
    }

    #[test]
    fn connection_error_detection() {
        let status = tonic::Status::unavailable("transport error: connection refused");
        assert!(is_connection_error(&status));

        let status = tonic::Status::unavailable("connection reset by peer");
        assert!(is_connection_error(&status));

        let status = tonic::Status::unavailable("broken pipe");
        assert!(is_connection_error(&status));

        let status = tonic::Status::unavailable("some other error");
        assert!(!is_connection_error(&status));
    }

    #[test]
    fn policy_exhausts_retries() {
        let config = RetryConfig {
            max_retries: 2,
            base_backoff_ms: 10,
        };
        let mut policy = GrpcRetryPolicy::new(&config);
        let mut req = ();

        // First retry should succeed
        let mut result: Result<(), tonic::Status> = Err(tonic::Status::unavailable("down"));
        assert!(
            policy.retry(&mut req, &mut result).is_some(),
            "should retry on first failure"
        );

        // Second retry should succeed
        let mut result: Result<(), tonic::Status> = Err(tonic::Status::unavailable("down"));
        assert!(
            policy.retry(&mut req, &mut result).is_some(),
            "should retry on second failure"
        );

        // Third attempt exhausted
        let mut result: Result<(), tonic::Status> = Err(tonic::Status::unavailable("down"));
        assert!(
            policy.retry(&mut req, &mut result).is_none(),
            "should not retry after max_retries exhausted"
        );
    }

    #[test]
    fn policy_no_retry_on_success() {
        let config = RetryConfig {
            max_retries: 3,
            base_backoff_ms: 10,
        };
        let mut policy = GrpcRetryPolicy::new(&config);
        let mut req = ();
        let mut result: Result<(), tonic::Status> = Ok(());
        assert!(
            policy.retry(&mut req, &mut result).is_none(),
            "should not retry on success"
        );
    }

    #[test]
    fn policy_no_retry_on_permanent_error() {
        let config = RetryConfig {
            max_retries: 3,
            base_backoff_ms: 10,
        };
        let mut policy = GrpcRetryPolicy::new(&config);
        let mut req = ();

        let mut result: Result<(), tonic::Status> =
            Err(tonic::Status::invalid_argument("bad input"));
        assert!(
            policy.retry(&mut req, &mut result).is_none(),
            "should not retry InvalidArgument"
        );
    }

    #[test]
    fn policy_no_retry_on_not_found() {
        let config = RetryConfig {
            max_retries: 3,
            base_backoff_ms: 10,
        };
        let mut policy = GrpcRetryPolicy::new(&config);
        let mut req = ();

        let mut result: Result<(), tonic::Status> = Err(tonic::Status::not_found("missing"));
        assert!(
            policy.retry(&mut req, &mut result).is_none(),
            "should not retry NotFound"
        );
    }

    #[test]
    fn policy_no_retry_on_permission_denied() {
        let config = RetryConfig {
            max_retries: 3,
            base_backoff_ms: 10,
        };
        let mut policy = GrpcRetryPolicy::new(&config);
        let mut req = ();

        let mut result: Result<(), tonic::Status> =
            Err(tonic::Status::permission_denied("forbidden"));
        assert!(
            policy.retry(&mut req, &mut result).is_none(),
            "should not retry PermissionDenied"
        );
    }

    #[test]
    fn policy_retries_connection_error() {
        let config = RetryConfig {
            max_retries: 3,
            base_backoff_ms: 10,
        };
        let mut policy = GrpcRetryPolicy::new(&config);
        let mut req = ();

        let mut result: Result<(), tonic::Status> = Err(tonic::Status::unavailable(
            "transport error: connection refused",
        ));
        assert!(
            policy.retry(&mut req, &mut result).is_some(),
            "should retry on connection refused"
        );
    }

    #[test]
    fn policy_clone_request() {
        let config = RetryConfig {
            max_retries: 1,
            base_backoff_ms: 10,
        };
        let mut policy = GrpcRetryPolicy::new(&config);
        let req = 42u32;
        let cloned =
            <GrpcRetryPolicy as Policy<u32, (), tonic::Status>>::clone_request(&mut policy, &req);
        assert_eq!(cloned, Some(42));
    }

    #[tokio::test]
    async fn retry_channel_creates_successfully() {
        let channel = RetryChannel::new("http://localhost:42070", RetryConfig::default());
        assert!(channel.is_ok());
    }

    #[test]
    fn retry_channel_invalid_addr() {
        let channel = RetryChannel::new("not a valid uri\n\n", RetryConfig::default());
        assert!(channel.is_err());
    }
}
