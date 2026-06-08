use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub(crate) const PROVIDER_RETRY_BASE_DELAY_MS: u64 = 500;
pub(crate) const PROVIDER_RETRY_CAP_MS: u64 = 4_000;

pub(crate) fn compute_provider_retry_delay_ms(
    base_delay_ms: u64,
    attempt: u32,
    jitter_seed: u64,
    cap_ms: u64,
) -> u64 {
    if base_delay_ms == 0 {
        return 0;
    }
    let base = base_delay_ms.saturating_mul(2u64.saturating_pow(attempt));
    let jitter_pct = 80 + (jitter_seed % 41);
    let jittered = base.saturating_mul(jitter_pct) / 100;
    jittered.min(cap_ms)
}

pub(crate) fn provider_retry_delay_with(base_delay_ms: u64, attempt: u32, cap_ms: u64) -> Duration {
    let jitter_seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::from(d.subsec_nanos()))
        .unwrap_or(20);
    Duration::from_millis(compute_provider_retry_delay_ms(
        base_delay_ms,
        attempt,
        jitter_seed,
        cap_ms,
    ))
}

pub(crate) fn provider_retry_delay(attempt: u32) -> Duration {
    provider_retry_delay_with(PROVIDER_RETRY_BASE_DELAY_MS, attempt, PROVIDER_RETRY_CAP_MS)
}

#[cfg(test)]
mod tests {
    use super::{
        compute_provider_retry_delay_ms, PROVIDER_RETRY_BASE_DELAY_MS, PROVIDER_RETRY_CAP_MS,
    };

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
                PROVIDER_RETRY_CAP_MS,
            ),
            PROVIDER_RETRY_CAP_MS
        );
        assert_eq!(
            compute_provider_retry_delay_ms(0, 10, 20, PROVIDER_RETRY_CAP_MS),
            0
        );
    }
}
