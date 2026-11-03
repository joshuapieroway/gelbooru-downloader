use std::time::Duration;
use tokio::time::sleep;

pub async fn with_retry<F, Fut, T, E>(max_retries: u32, mut f: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Debug,
{
    let mut attempt = 0;
    loop {
        match f().await {
            Ok(v) => return Ok(v),
            Err(e) => {
                attempt += 1;
                if attempt > max_retries {
                    return Err(e);
                }
                let backoff = Duration::from_millis(200 * (1 << attempt.min(6)));
                tracing::warn!(?e, attempt, "retrying after backoff");
                sleep(backoff).await;
            }
        }
    }
}
