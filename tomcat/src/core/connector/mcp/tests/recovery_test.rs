use super::*;

#[tokio::test(start_paused = true)]
async fn attempts_deadline_and_stability_are_separate() {
    let start = Instant::now();
    let mut budget = RecoveryBudget::new(30_000, MAX_ATTEMPTS).unwrap();
    assert_eq!(budget.remaining_ms(), 111_250);
    assert_eq!(budget.next(start), Some((1, Duration::ZERO)));
    tokio::time::advance(Duration::from_secs(10)).await;
    assert!(!stable(Some(start), Instant::now()));
    assert_eq!(
        budget.next(Instant::now()),
        Some((2, Duration::from_millis(250)))
    );
    assert_eq!(
        budget.next(Instant::now()),
        Some((3, Duration::from_secs(1)))
    );
    assert!(budget.next(Instant::now()).is_none());
    tokio::time::advance(Duration::from_secs(20)).await;
    assert!(stable(Some(start), Instant::now()));
    assert!(
        budget.next(Instant::now()).is_none(),
        "only a new recovery cycle may replenish attempts"
    );
    assert!(RecoveryBudget::new(u64::MAX, MAX_ATTEMPTS).is_err());
    let mut expired = RecoveryBudget::new(1, MAX_ATTEMPTS).unwrap();
    tokio::time::advance(Duration::from_secs(22)).await;
    assert!(expired.next(Instant::now()).is_none());
}
