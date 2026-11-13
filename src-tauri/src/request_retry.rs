//! Immediate, bounded retries for transient failures. Never retries user cancellation
//! (dropping the future), validation errors, quota errors, or responses requiring a wait.
use std::future::Future;

pub const MAX_ATTEMPTS: usize = 3;

#[derive(Debug)]
pub struct RequestError {
    pub message: String,
    pub retryable: bool,
}
impl From<String> for RequestError {
    fn from(message: String) -> Self {
        Self {
            message,
            retryable: false,
        }
    }
}
impl From<&str> for RequestError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}
pub fn transient(message: impl Into<String>) -> RequestError {
    RequestError {
        message: message.into(),
        retryable: true,
    }
}
pub fn transport(error: reqwest::Error, message: &str) -> RequestError {
    crate::diagnostics::record(
        "error",
        "network",
        "transport_error",
        serde_json::json!({"message":message,"error":format!("{error:?}")}),
    );
    RequestError {
        message: message.into(),
        retryable: error.is_timeout()
            || error.is_connect()
            || error.is_body()
            || error.is_request() && !error.is_builder(),
    }
}
pub fn http_retryable(response: &reqwest::Response) -> bool {
    matches!(response.status().as_u16(), 408 | 500 | 502 | 503 | 504)
        && !response
            .headers()
            .contains_key(reqwest::header::RETRY_AFTER)
}
pub async fn run<T, F, Fut>(category: &'static str, mut operation: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RequestError>>,
{
    let mut trace = crate::diagnostics::RequestTrace::new(category);
    for attempt in 0..MAX_ATTEMPTS {
        match operation().await {
            Ok(value) => {
                trace.success(attempt + 1, None);
                return Ok(value);
            }
            Err(error) if error.retryable && attempt + 1 < MAX_ATTEMPTS => {
                trace.retry(attempt + 1, &error.message);
                continue;
            }
            Err(error) => {
                trace.failure(attempt + 1, &error.message);
                return Err(error.message);
            }
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bounded_immediate_retries_and_fatal_errors() {
        let mut count = 0;
        let result: Result<(), _> = run("test", || {
            count += 1;
            std::future::ready(Err(transient("offline")))
        })
        .await;
        assert_eq!(count, 3);
        assert_eq!(result.unwrap_err(), "offline");
        count = 0;
        let result: Result<(), _> = run("test", || {
            count += 1;
            std::future::ready(Err("invalid key".into()))
        })
        .await;
        assert_eq!(count, 1);
        assert_eq!(result.unwrap_err(), "invalid key");
    }
    #[tokio::test]
    async fn dropping_retry_future_cancels_pending_attempt() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(10),
            run("test", || {
                let number = calls.fetch_add(1, Ordering::SeqCst);
                async move {
                    if number == 0 {
                        Err(transient("connection reset"))
                    } else {
                        std::future::pending::<Result<(), RequestError>>().await
                    }
                }
            }),
        )
        .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
