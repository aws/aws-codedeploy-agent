//! Deadline-based thread/task joiner.
//!
//! Takes a timeout at creation, computes a deadline, then joins multiple tasks
//! sequentially — each getting only the *remaining* time. This lets the caller
//! distinguish "process timed out" from "stdout/stderr streams didn't close in
//! time" (`OUTPUTS_LEFT_OPEN`).
//!
//! See [`DeadlineJoiner`] for the async implementation.

use std::time::Duration;
use tokio::time::Instant;

/// A shared deadline that tracks remaining time across sequential joins.
#[derive(Debug)]
pub struct DeadlineJoiner {
    deadline: Instant,
}

impl DeadlineJoiner {
    /// Create a joiner with a deadline of `now + timeout`.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self { deadline: Instant::now() + timeout }
    }

    /// Time remaining until the deadline. Returns `Duration::ZERO` if past.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.deadline.saturating_duration_since(Instant::now())
    }

    /// Wait for a future up to the remaining deadline.
    ///
    /// Returns `Ok(value)` if the future completes in time, `Err(())` if the
    /// deadline is exceeded.
    ///
    /// # Errors
    /// Returns `Err(())` if the deadline is exceeded before the future completes.
    pub async fn join<F, T>(&self, future: F) -> Result<T, ()>
    where
        F: std::future::Future<Output = T>,
    {
        tokio::time::timeout_at(self.deadline, future).await.map_err(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn join_completes_before_deadline() {
        let joiner = DeadlineJoiner::new(Duration::from_secs(5));
        let result = joiner.join(async { 42 }).await;
        assert_eq!(result, Ok(42));
    }

    #[tokio::test]
    async fn join_exceeds_deadline() {
        let joiner = DeadlineJoiner::new(Duration::from_millis(10));
        let result = joiner.join(tokio::time::sleep(Duration::from_secs(5))).await;
        assert_eq!(result, Err(()));
    }

    #[tokio::test]
    async fn remaining_decreases_over_time() {
        let joiner = DeadlineJoiner::new(Duration::from_millis(200));
        let before = joiner.remaining();
        tokio::time::sleep(Duration::from_millis(50)).await;
        let after = joiner.remaining();
        assert!(after < before, "remaining should decrease: {before:?} -> {after:?}");
    }

    #[tokio::test]
    async fn remaining_returns_zero_after_deadline() {
        let joiner = DeadlineJoiner::new(Duration::from_millis(10));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(joiner.remaining(), Duration::ZERO);
    }

    #[tokio::test]
    async fn sequential_joins_share_deadline() {
        let joiner = DeadlineJoiner::new(Duration::from_millis(100));

        // First join uses ~50ms.
        let r1 = joiner.join(tokio::time::sleep(Duration::from_millis(50))).await;
        assert!(r1.is_ok(), "first join should succeed");

        // Second join gets remaining ~50ms — a 200ms sleep should fail.
        let r2 = joiner.join(tokio::time::sleep(Duration::from_millis(200))).await;
        assert!(r2.is_err(), "second join should exceed deadline");
    }
}
