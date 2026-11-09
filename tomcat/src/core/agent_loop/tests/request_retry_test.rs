use super::*;

#[test]
fn retry_delay_is_bounded_and_allows_zero_delay() {
    assert_eq!(compute_retry_delay_ms(500, 1, 20), 0);
    assert_eq!(compute_retry_delay_ms(0, 10, 20), 0);
    assert_eq!(compute_retry_delay_ms(500, 20, 40), 30_000);
    assert_eq!(compute_retry_delay_ms(u64::MAX, u32::MAX, 40), 30_000);
}
