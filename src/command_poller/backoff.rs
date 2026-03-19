//! Exponential backoff for the polling loop.
//!
//! Ruby source: `lib/instance_agent/agent/base.rb` — `run` method.
//!
//! Formula: `floor(1.2675^error_count * 90.0 / 1.2675^10)`
//! Error count is capped at 10, giving a max sleep of 90 seconds.
//! Resets to 0 on any successful poll cycle.

use std::time::Duration;

/// Ruby: `1.2675`
const BASE: f64 = 1.2675;
/// Ruby: `90.0` — maximum backoff in seconds.
const MAX_BACKOFF_SECS: f64 = 90.0;
/// Ruby: error count capped at 10.
const MAX_ERROR_COUNT: u32 = 10;
/// Tracks consecutive errors and computes backoff sleep durations.
///
/// Ruby: `@error_count` in `base.rb`.
#[derive(Debug, Default)]
pub struct PollBackoff {
    error_count: u32,
}

impl PollBackoff {
    #[must_use]
    pub fn new() -> Self {
        Self { error_count: 0 }
    }

    /// Record a successful poll cycle. Resets backoff to zero.
    ///
    /// Ruby: `@error_count = 0` after successful `perform`.
    pub fn reset(&mut self) {
        self.error_count = 0;
    }

    /// Record a poll error. Increments the error count (capped at 10).
    ///
    /// Ruby: `@error_count = @error_count.to_i + 1` then `if @error_count > 10; @error_count = 10; end`
    pub fn record_error(&mut self) {
        self.error_count = (self.error_count + 1).min(MAX_ERROR_COUNT);
    }

    /// Compute the sleep duration, subtracting elapsed time.
    ///
    /// Returns `None` if no backoff is needed (`error_count` == 0 or elapsed exceeds backoff).
    ///
    /// Ruby: `elapsed_time = (Time.now - start_time).ceil`
    /// Ruby: `sleep_time = backoff_time - elapsed_time; sleep sleep_time if sleep_time > 0`
    #[must_use]
    pub fn sleep_duration(&self, elapsed: Duration) -> Option<Duration> {
        if self.error_count == 0 {
            return None;
        }
        let backoff_secs = backoff_seconds(self.error_count);
        // Ruby uses .ceil on elapsed time — round up so we don't over-sleep.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let elapsed_secs = elapsed.as_secs_f64().ceil() as u64;
        if backoff_secs > elapsed_secs {
            Some(Duration::from_secs(backoff_secs - elapsed_secs))
        } else {
            None
        }
    }

    /// Current error count (for logging/testing).
    #[must_use]
    pub fn error_count(&self) -> u32 {
        self.error_count
    }
}

/// Ruby: `(((1.2675 ** @error_count) * (90.0 / (1.2675 ** 10)))).floor`
#[must_use]
fn backoff_seconds(error_count: u32) -> u64 {
    // Compute BASE^10 at runtime to match Ruby, which evaluates `1.2675 ** 10` each call.
    let base_pow_10 = BASE.powf(10.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let result =
        (BASE.powf(f64::from(error_count)) * (MAX_BACKOFF_SECS / base_pow_10)).floor() as u64;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_seconds_matches_ruby_values() {
        // Ruby: (1.2675 ** n) * (90.0 / (1.2675 ** 10))
        // Verified against Ruby 3.x output for all capped error counts.
        assert_eq!(backoff_seconds(0), 8);
        assert_eq!(backoff_seconds(1), 10);
        assert_eq!(backoff_seconds(2), 13);
        assert_eq!(backoff_seconds(3), 17);
        assert_eq!(backoff_seconds(4), 21);
        assert_eq!(backoff_seconds(5), 27);
        assert_eq!(backoff_seconds(6), 34);
        assert_eq!(backoff_seconds(7), 44);
        assert_eq!(backoff_seconds(8), 56);
        assert_eq!(backoff_seconds(9), 71);
        assert_eq!(backoff_seconds(10), 90);
    }

    #[test]
    fn error_count_caps_at_10() {
        let mut b = PollBackoff::new();
        for _ in 0..20 {
            b.record_error();
        }
        assert_eq!(b.error_count(), 10);
    }

    #[test]
    fn reset_clears_error_count() {
        let mut b = PollBackoff::new();
        b.record_error();
        b.record_error();
        assert_eq!(b.error_count(), 2);
        b.reset();
        assert_eq!(b.error_count(), 0);
    }

    #[test]
    fn sleep_duration_none_when_no_errors() {
        let b = PollBackoff::new();
        assert!(b.sleep_duration(Duration::ZERO).is_none());
    }

    #[test]
    fn sleep_duration_subtracts_elapsed() {
        let mut b = PollBackoff::new();
        b.record_error(); // backoff = 10s
        let dur = b.sleep_duration(Duration::from_secs(3));
        assert_eq!(dur, Some(Duration::from_secs(7)));
    }

    #[test]
    fn sleep_duration_none_when_elapsed_exceeds_backoff() {
        let mut b = PollBackoff::new();
        b.record_error(); // backoff = 10s
        assert!(b.sleep_duration(Duration::from_secs(15)).is_none());
    }

    #[test]
    fn sleep_duration_at_max_errors() {
        let mut b = PollBackoff::new();
        for _ in 0..10 {
            b.record_error();
        }
        let dur = b.sleep_duration(Duration::ZERO);
        assert_eq!(dur, Some(Duration::from_secs(90)));
    }

    #[test]
    fn default_creates_zero_errors() {
        let b = PollBackoff::default();
        assert_eq!(b.error_count(), 0);
    }
}
