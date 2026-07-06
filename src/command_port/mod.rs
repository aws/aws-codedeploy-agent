//! Command port — lightweight local TCP interface for agent management and debugging.
//!
//! Binds to `127.0.0.1` on a dynamic port, writes a discovery file with the
//! port and auth token, and accepts JSON commands over TCP.

pub mod auth;
pub mod commands;
pub mod router;
pub mod server;

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::thread::JoinHandle;
use std::time::Instant;
use tracing::info;

/// Shared agent state readable by command port handlers.
#[derive(Debug)]
pub struct AgentState {
    pub worker_pid: Option<u32>,
    pub worker_restarts: u32,
    pub worker_alive: bool,
    pub started_at: Instant,
}

impl AgentState {
    /// Human-readable status label.
    #[must_use]
    pub fn status_label(&self) -> &str {
        if self.worker_alive {
            "running"
        } else {
            "worker_down"
        }
    }
}

/// Start the command port in a background thread.
///
/// Returns the join handle and shared state. The caller should hold the
/// `Arc<RwLock<AgentState>>` and update it as deployments progress.
///
/// # Errors
/// Returns an error if binding or auth initialization fails.
pub fn start(discovery_path: &Path) -> std::io::Result<(JoinHandle<()>, Arc<RwLock<AgentState>>)> {
    let (listener, port) = server::bind()?;
    let auth = Arc::new(auth::Auth::init(discovery_path.to_path_buf(), port)?);
    let state = Arc::new(RwLock::new(AgentState {
        worker_pid: None,
        worker_restarts: 0,
        worker_alive: false,
        started_at: Instant::now(),
    }));
    let inject_dir: Arc<RwLock<Option<PathBuf>>> =
        Arc::new(RwLock::new(discovery_path.parent().map(Path::to_path_buf)));

    info!(port, "Command port starting");

    let serve_state = Arc::clone(&state);
    let serve_inject = Arc::clone(&inject_dir);
    let handle = std::thread::Builder::new()
        .name("command-port".into())
        .spawn(move || {
            server::serve(&listener, &auth, &serve_state, &serve_inject);
        })
        .map_err(|e| std::io::Error::other(format!("Failed to spawn command port thread: {e}")))?;

    Ok((handle, state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_state_idle() {
        let state = AgentState {
            worker_pid: None,
            worker_restarts: 0,
            worker_alive: false,
            started_at: Instant::now(),
        };
        assert_eq!(state.status_label(), "worker_down");
    }

    #[test]
    fn agent_state_executing() {
        let state = AgentState {
            worker_pid: Some(42),
            worker_restarts: 1,
            worker_alive: true,
            started_at: Instant::now(),
        };
        assert_eq!(state.status_label(), "running");
    }

    #[test]
    fn start_and_connect() {
        use std::io::{BufRead, BufReader, Write};

        let dir = tempfile::TempDir::new().unwrap();
        let discovery_path = dir.path().join(".command-port");
        let (_handle, _state) = start(&discovery_path).unwrap();

        // Read discovery file
        let content = std::fs::read_to_string(&discovery_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let port = u16::try_from(parsed["port"].as_u64().unwrap()).unwrap();
        let token = parsed["token"].as_str().unwrap();

        // Connect and ping
        let mut stream = std::net::TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        stream.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();

        let req = serde_json::json!({"command": "ping", "token": token}).to_string();
        writeln!(stream, "{req}").unwrap();

        let mut reader = BufReader::new(&stream);
        let mut resp = String::new();
        reader.read_line(&mut resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed["ok"], true);
    }
}
