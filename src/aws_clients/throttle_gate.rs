//! Process-wide throttle circuit breaker.
//!
//! When any thread receives a throttling response (HTTP 429), it trips the gate
//! with a backoff deadline. All subsequent callers wait until the deadline before
//! making further requests — collectively respecting the rate limit instead of
//! each thread retrying independently.
//!
//! States:
//! - **Closed** (normal): requests proceed immediately.
//! - **Open** (throttled): requests wait until `throttled_until`.
//! - **Half-open**: after the deadline, one probe goes through; success closes
//!   the gate, another throttle extends the deadline.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use tracing::{debug, warn};

/// Maximum backoff duration for a single throttle event.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// Initial backoff on first throttle.
const INITIAL_BACKOFF: Duration = Duration::from_secs(1);

/// Process-wide throttle gate shared across all client instances.
#[derive(Debug)]
pub struct ThrottleGate {
    state: Mutex<GateState>,
}

#[derive(Debug)]
struct GateState {
    /// When the throttle expires. `None` = gate closed (no throttle).
    throttled_until: Option<Instant>,
    /// Current backoff level (doubles on each consecutive throttle).
    backoff: Duration,
}

impl ThrottleGate {
    /// Create a new gate in the closed (normal) state.
    #[must_use]
    pub fn new() -> Self {
        Self { state: Mutex::new(GateState { throttled_until: None, backoff: INITIAL_BACKOFF }) }
    }

    /// Trip the gate — called when a throttle response is received.
    /// Only escalates backoff if the previous deadline has already expired,
    /// preventing concurrent threads from multiplying the backoff for a single event.
    pub fn trip(&self) {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        // Only set a new deadline + escalate if not already throttled.
        if state.throttled_until.is_none_or(|until| now >= until) {
            let deadline = now + state.backoff;
            warn!(
                backoff_ms = u64::try_from(state.backoff.as_millis()).unwrap_or(u64::MAX),
                "Throttle gate tripped, backing off"
            );
            state.throttled_until = Some(deadline);
            state.backoff = (state.backoff * 2).min(MAX_BACKOFF);
        }
    }

    /// Wait until the gate is open. Returns immediately if not throttled.
    /// Callers invoke this before making an API request.
    /// Adds random jitter (0–25% of wait) to prevent thundering herd when
    /// multiple threads wake from the same deadline simultaneously.
    pub fn wait_if_throttled(&self) {
        let deadline = {
            let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            state.throttled_until
        };
        if let Some(until) = deadline {
            let now = Instant::now();
            if now < until {
                let base_wait = until - now;
                let jitter = Self::jitter(base_wait);
                let wait = base_wait + jitter;
                debug!(
                    wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                    "Waiting for throttle gate to open"
                );
                std::thread::sleep(wait);
            }
        }
    }

    /// Generate random jitter: 0–25% of the base duration.
    fn jitter(base: Duration) -> Duration {
        let quarter_ms = u64::try_from(base.as_millis()).unwrap_or(u64::MAX) / 4;
        if quarter_ms == 0 {
            return Duration::ZERO;
        }
        let mut buf = [0u8; 8];
        // getrandom never fails on supported platforms; fallback to 0 jitter.
        if getrandom::getrandom(&mut buf).is_err() {
            return Duration::ZERO;
        }
        let random_val = u64::from_le_bytes(buf);
        Duration::from_millis(random_val % quarter_ms)
    }

    /// Non-blocking check: is the gate currently throttled?
    /// Used by the poll loop to decide whether to skip the immediate re-poll.
    #[must_use]
    pub fn is_throttled(&self) -> bool {
        let state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        state.throttled_until.is_some_and(|until| Instant::now() < until)
    }

    /// Reset the gate to closed — called after a successful request (half-open → closed).
    pub fn reset(&self) {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.throttled_until.is_some() {
            debug!("Throttle gate reset (half-open → closed)");
            state.throttled_until = None;
            state.backoff = INITIAL_BACKOFF;
        }
    }
}

impl Default for ThrottleGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl ThrottleGate {
    /// Set the backoff duration for testing (avoids 1s+ waits in tests).
    pub(crate) fn set_backoff_for_test(&self, backoff: Duration) {
        let mut state = self.state.lock().unwrap();
        state.backoff = backoff;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_gate_is_not_throttled() {
        let gate = ThrottleGate::new();
        assert!(!gate.is_throttled());
    }

    #[test]
    fn trip_sets_throttled() {
        let gate = ThrottleGate::new();
        gate.trip();
        assert!(gate.is_throttled());
    }

    #[test]
    fn reset_clears_throttle() {
        let gate = ThrottleGate::new();
        gate.trip();
        assert!(gate.is_throttled());
        gate.reset();
        assert!(!gate.is_throttled());
    }

    #[test]
    fn wait_if_throttled_returns_immediately_when_not_tripped() {
        let gate = ThrottleGate::new();
        let start = Instant::now();
        gate.wait_if_throttled();
        assert!(start.elapsed() < Duration::from_millis(50));
    }

    #[test]
    fn wait_if_throttled_blocks_until_deadline() {
        let gate = ThrottleGate::new();
        // Override state to a short deadline for testing
        {
            let mut state = gate.state.lock().unwrap();
            state.throttled_until = Some(Instant::now() + Duration::from_millis(100));
        }
        let start = Instant::now();
        gate.wait_if_throttled();
        // Wait is 100ms + up to 25ms jitter
        assert!(start.elapsed() >= Duration::from_millis(90));
        assert!(start.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn backoff_doubles_on_consecutive_trips() {
        let gate = ThrottleGate::new();
        gate.trip();
        let b1 = gate.state.lock().unwrap().backoff;
        gate.reset();
        // After reset, backoff resets to initial
        let b_after_reset = gate.state.lock().unwrap().backoff;
        assert_eq!(b_after_reset, INITIAL_BACKOFF);

        // Without reset, backoff doubles only when the deadline expires between trips
        let gate2 = ThrottleGate::new();
        gate2.trip(); // backoff becomes 2s, deadline set
        // Expire the deadline so the next trip escalates
        {
            let mut state = gate2.state.lock().unwrap();
            state.throttled_until =
                Some(Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
        }
        gate2.trip(); // backoff becomes 4s
        let b = gate2.state.lock().unwrap().backoff;
        assert_eq!(b, Duration::from_secs(4));
        let _ = b1; // suppress unused warning
    }

    #[test]
    fn backoff_caps_at_max() {
        let gate = ThrottleGate::new();
        // Trip many times, expiring deadline between each so escalation occurs
        for _ in 0..20 {
            {
                let mut state = gate.state.lock().unwrap();
                state.throttled_until =
                    Some(Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
            }
            gate.trip();
        }
        let b = gate.state.lock().unwrap().backoff;
        assert_eq!(b, MAX_BACKOFF);
    }

    #[test]
    fn is_throttled_false_after_deadline_passes() {
        let gate = ThrottleGate::new();
        {
            let mut state = gate.state.lock().unwrap();
            // Set deadline in the past
            state.throttled_until =
                Some(Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
        }
        assert!(!gate.is_throttled());
    }

    #[test]
    fn concurrent_access_does_not_panic() {
        use std::sync::Arc;
        let gate = Arc::new(ThrottleGate::new());
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let g = Arc::clone(&gate);
                std::thread::spawn(move || {
                    if i % 2 == 0 {
                        g.trip();
                    } else {
                        g.wait_if_throttled();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
    }

    #[test]
    fn concurrent_trips_do_not_escalate_backoff() {
        // Multiple trips while the gate is already throttled should NOT double backoff
        let gate = ThrottleGate::new();
        gate.trip(); // sets deadline, backoff becomes 2s
        let b_after_first = gate.state.lock().unwrap().backoff;
        assert_eq!(b_after_first, Duration::from_secs(2));

        // Simulate concurrent threads all tripping (deadline still in the future)
        gate.trip();
        gate.trip();
        gate.trip();

        // Backoff should still be 2s, not 16s
        let b_after_concurrent = gate.state.lock().unwrap().backoff;
        assert_eq!(b_after_concurrent, Duration::from_secs(2));
    }
}
