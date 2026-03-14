use std::future::Future;
use std::time::Duration;

use tracing::warn;

/// Retry an async fallible operation up to `max_attempts` times with a fixed delay between attempts.
///
/// On transient failure the `label` is logged at WARN level with the attempt number.
/// Returns the first `Ok` or the last `Err` after all attempts are exhausted.
pub async fn retry<F, Fut, T, E>(
    max_attempts: u32,
    delay: Duration,
    label: &str,
    f: F,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut last_err: Option<E> = None;
    for attempt in 0..max_attempts {
        if attempt > 0 {
            warn!(label, attempt, "retrying after transient failure");
            tokio::time::sleep(delay).await;
        }
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.expect("max_attempts must be >= 1"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    async fn fail_then_succeed(counter: &AtomicU32) -> Result<&'static str, String> {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        if n < 2 {
            Err("transient".into())
        } else {
            Ok("recovered")
        }
    }

    async fn always_fail(counter: &AtomicU32) -> Result<(), String> {
        let n = counter.fetch_add(1, Ordering::SeqCst);
        Err(format!("fail-{n}"))
    }

    #[tokio::test]
    async fn succeeds_on_first_attempt() {
        let result: Result<&str, String> =
            retry(3, Duration::from_millis(1), "test", || async { Ok("ok") }).await;
        assert_eq!(result.unwrap(), "ok");
    }

    #[tokio::test]
    async fn succeeds_after_retries() {
        let counter = AtomicU32::new(0);
        let result = retry(3, Duration::from_millis(1), "test", || {
            fail_then_succeed(&counter)
        })
        .await;
        assert_eq!(result.unwrap(), "recovered");
        assert_eq!(counter.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn returns_last_error_after_exhausting_attempts() {
        let counter = AtomicU32::new(0);
        let result = retry(2, Duration::from_millis(1), "test", || {
            always_fail(&counter)
        })
        .await;
        assert_eq!(result.unwrap_err(), "fail-1");
    }
}
