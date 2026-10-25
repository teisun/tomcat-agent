use super::*;

#[test]
fn unattended_mode_allows_ten_attempts() {
    assert_eq!(
        retry_attempt_budget(true, DEFAULT_AGENT_MAX_ATTEMPTS),
        UNATTENDED_TRANSIENT_MAX_ATTEMPTS
    );
    assert_eq!(
        retry_attempt_budget(false, DEFAULT_AGENT_MAX_ATTEMPTS),
        DEFAULT_AGENT_MAX_ATTEMPTS
    );
    assert_eq!(
        compute_retry_delay_with_cap_ms(500, 20, 40, UNATTENDED_RETRY_DELAY_CAP_MS),
        UNATTENDED_RETRY_DELAY_CAP_MS
    );
}
