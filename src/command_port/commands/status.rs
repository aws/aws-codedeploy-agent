//! Status command — returns current agent state.

use super::AgentState;
use serde_json::{Value, json};
use std::sync::{Arc, RwLock};

/// Returns the current agent state.
#[must_use]
pub fn handle(_args: &Value, state: &Arc<RwLock<AgentState>>) -> Value {
    let state = state.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    json!({
        "ok": true,
        "data": {
            "status": state.status_label(),
            "worker_pid": state.worker_pid,
            "worker_alive": state.worker_alive,
            "worker_restarts": state.worker_restarts,
            "uptime_secs": state.started_at.elapsed().as_secs(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn running_state() -> Arc<RwLock<AgentState>> {
        Arc::new(RwLock::new(AgentState {
            worker_pid: Some(1234),
            worker_restarts: 0,
            worker_alive: true,
            started_at: Instant::now(),
        }))
    }

    fn down_state() -> Arc<RwLock<AgentState>> {
        Arc::new(RwLock::new(AgentState {
            worker_pid: None,
            worker_restarts: 3,
            worker_alive: false,
            started_at: Instant::now(),
        }))
    }

    #[test]
    fn status_running() {
        let resp = handle(&json!({}), &running_state());
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["data"]["status"], "running");
        assert_eq!(resp["data"]["worker_pid"], 1234);
        assert_eq!(resp["data"]["worker_alive"], true);
    }

    #[test]
    fn status_worker_down() {
        let resp = handle(&json!({}), &down_state());
        assert_eq!(resp["data"]["status"], "worker_down");
        assert!(resp["data"]["worker_pid"].is_null());
        assert_eq!(resp["data"]["worker_restarts"], 3);
    }

    #[test]
    fn uptime_is_positive() {
        let resp = handle(&json!({}), &running_state());
        assert!(resp["data"]["uptime_secs"].as_u64().is_some());
    }
}
