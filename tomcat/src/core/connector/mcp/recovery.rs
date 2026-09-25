//! Per-source recovery policy. A successful but short-lived connection does
//! not replenish attempts; request arguments never enter this state.
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::call::CLEANUP_GRACE;
use super::failure::{FailureKind, McpFailure};

pub(crate) const MAX_ATTEMPTS: u8 = 3;
pub(crate) const STABLE_PERIOD: Duration = Duration::from_secs(30);

#[derive(Clone, Debug)]
pub(crate) struct RecoveryBudget {
    pub deadline: Instant,
    pub used: u8,
    pub limit: u8,
    pub refresh_used: Arc<AtomicBool>,
}

impl RecoveryBudget {
    pub fn new(startup_ms: u64, limit: u8) -> Result<Self, McpFailure> {
        let millis = startup_ms
            .checked_mul(u64::from(limit))
            .and_then(|ms| ms.checked_add(1250))
            .and_then(|ms| {
                ms.checked_add((u64::from(limit) + 1) * CLEANUP_GRACE.as_millis() as u64)
            })
            .ok_or_else(|| {
                McpFailure::new(FailureKind::Configuration, "recovery budget overflow")
            })?;
        let deadline = Instant::now()
            .checked_add(Duration::from_millis(millis))
            .ok_or_else(|| {
                McpFailure::new(FailureKind::Configuration, "recovery deadline overflow")
            })?;
        Ok(Self {
            deadline,
            used: 0,
            limit,
            refresh_used: Arc::default(),
        })
    }
    pub fn next(&mut self, now: Instant) -> Option<(u8, Duration)> {
        if self.used >= self.limit || now >= self.deadline {
            return None;
        }
        let delay = match self.used {
            0 => Duration::ZERO,
            1 => Duration::from_millis(250),
            _ => Duration::from_secs(1),
        };
        self.used += 1;
        Some((self.used, delay))
    }
    pub fn remaining_ms(&self) -> u64 {
        self.deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

pub(crate) struct RecoveryRun {
    pub cancel: CancellationToken,
    pub done: CancellationToken,
    pub cleanup: TaskTracker,
    published: CancellationToken,
    outcome: Mutex<Option<Result<(), McpFailure>>>,
}

impl RecoveryRun {
    pub fn new() -> Self {
        Self {
            cancel: CancellationToken::new(),
            done: CancellationToken::new(),
            published: CancellationToken::new(),
            cleanup: TaskTracker::new(),
            outcome: Mutex::new(None),
        }
    }
    pub fn finish(&self, outcome: Result<(), McpFailure>) {
        let mut slot = self.outcome.lock();
        if slot.is_none() {
            *slot = Some(outcome);
            self.published.cancel();
        }
    }
    pub fn is_active(&self) -> bool {
        !self.done.is_cancelled() && !matches!(*self.outcome.lock(), Some(Ok(())))
    }
    pub async fn wait(&self) -> Result<(), McpFailure> {
        self.published.cancelled().await;
        self.outcome
            .lock()
            .clone()
            .unwrap_or_else(|| Err(McpFailure::new(FailureKind::Unknown, "recovery owner")))
    }
}

pub(crate) fn stable(ready_since: Option<Instant>, now: Instant) -> bool {
    ready_since.is_some_and(|since| now.saturating_duration_since(since) >= STABLE_PERIOD)
}

#[cfg(test)]
mod tests {
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
}
