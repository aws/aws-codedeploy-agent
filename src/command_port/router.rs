//! Routes command names to their handler functions.

use super::AgentState;
use super::commands::{inject, ping, status};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Dispatch a command by name.
#[must_use]
pub fn route(
    command: &str,
    args: &Value,
    state: &Arc<RwLock<AgentState>>,
    inject_dir: &Arc<RwLock<Option<PathBuf>>>,
) -> Value {
    match command {
        "ping" => ping::handle(args),
        "status" => status::handle(args, state),
        "inject" => inject::handle(args, inject_dir),
        other => json!({"ok": false, "error": format!("unknown command: {other}")}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn test_state() -> Arc<RwLock<AgentState>> {
        Arc::new(RwLock::new(AgentState {
            worker_pid: None,
            worker_restarts: 0,
            worker_alive: false,
            started_at: Instant::now(),
        }))
    }

    fn no_inject() -> Arc<RwLock<Option<PathBuf>>> {
        Arc::new(RwLock::new(None))
    }

    #[test]
    fn route_ping() {
        let resp = route("ping", &json!({}), &test_state(), &no_inject());
        assert_eq!(resp["ok"], true);
    }

    #[test]
    fn route_status() {
        let resp = route("status", &json!({}), &test_state(), &no_inject());
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["data"]["status"], "worker_down");
    }

    #[test]
    fn route_unknown() {
        let resp = route("bogus", &json!({}), &test_state(), &no_inject());
        assert_eq!(resp["ok"], false);
        assert!(resp["error"].as_str().unwrap().contains("unknown command: bogus"));
    }

    #[test]
    fn route_inject_not_configured() {
        let resp = route(
            "inject",
            &json!({"host_command_identifier": "c", "deployment_execution_id": "d", "command_name": "Install"}),
            &test_state(),
            &no_inject(),
        );
        assert_eq!(resp["ok"], false);
        assert!(resp["error"].as_str().unwrap().contains("not configured"));
    }
}
