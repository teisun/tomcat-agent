use crate::core::llm::retry_delay::{
    compute_provider_retry_delay_ms, is_provider_retry_cancelled, sleep_provider_retry_delay,
    with_provider_retry_cancel, PROVIDER_RETRY_BASE_DELAY_MS, PROVIDER_RETRY_CAP_MS,
};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[test]
fn provider_retry_delay_uses_jitter_window() {
    let base = PROVIDER_RETRY_BASE_DELAY_MS;
    assert_eq!(
        compute_provider_retry_delay_ms(base, 0, 0, PROVIDER_RETRY_CAP_MS),
        400
    );
    assert_eq!(
        compute_provider_retry_delay_ms(base, 0, 20, PROVIDER_RETRY_CAP_MS),
        500
    );
    assert_eq!(
        compute_provider_retry_delay_ms(base, 0, 40, PROVIDER_RETRY_CAP_MS),
        600
    );
    assert_eq!(
        compute_provider_retry_delay_ms(base, 1, 0, PROVIDER_RETRY_CAP_MS),
        800
    );
    assert_eq!(
        compute_provider_retry_delay_ms(base, 1, 40, PROVIDER_RETRY_CAP_MS),
        1200
    );
}

#[test]
fn provider_retry_delay_caps_and_saturates_large_attempts() {
    assert_eq!(
        compute_provider_retry_delay_ms(
            PROVIDER_RETRY_BASE_DELAY_MS,
            63,
            40,
            PROVIDER_RETRY_CAP_MS
        ),
        PROVIDER_RETRY_CAP_MS
    );
    assert_eq!(
        compute_provider_retry_delay_ms(0, 10, 20, PROVIDER_RETRY_CAP_MS),
        0
    );
}

#[tokio::test(start_paused = true)]
async fn scoped_retry_sleep_wakes_immediately_on_cancel() {
    let cancel = CancellationToken::new();
    let wait = tokio::spawn(with_provider_retry_cancel(cancel.clone(), async move {
        sleep_provider_retry_delay(Duration::from_secs(4)).await
    }));
    tokio::task::yield_now().await;
    cancel.cancel();
    let result = wait.await.expect("join");
    assert!(matches!(result, Err(err) if is_provider_retry_cancelled(&err)));
}

#[tokio::test(start_paused = true)]
async fn unscoped_retry_sleep_waits_full_delay() {
    let wait = tokio::spawn(async { sleep_provider_retry_delay(Duration::from_secs(4)).await });
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(3)).await;
    assert!(
        !wait.is_finished(),
        "unscoped sleep should still be waiting"
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(wait.await.expect("join").is_ok());
}
