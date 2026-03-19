//! @risk high
//!
//! TCP listener, auth check, and connection handling.

use super::AgentState;
use super::auth::Auth;
use super::router;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tracing::{debug, error, info, warn};

const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REQUEST_BYTES: u64 = 64 * 1024; // 64 KiB
const MAX_CONNECTIONS: usize = 8;

/// Bind to a dynamic port on localhost and return the listener + assigned port.
///
/// # Errors
/// Returns an error if binding fails.
pub fn bind() -> io::Result<(TcpListener, u16)> {
    // @risk critical — must be 127.0.0.1, never 0.0.0.0
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    info!(port, "Command port bound");
    Ok((listener, port))
}

/// Accept connections in a loop. Blocks the calling thread.
pub fn serve(
    listener: &TcpListener,
    auth: &Arc<Auth>,
    state: &Arc<RwLock<AgentState>>,
    inject_dir: &Arc<RwLock<Option<std::path::PathBuf>>>,
) {
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                if active.load(std::sync::atomic::Ordering::Relaxed) >= MAX_CONNECTIONS {
                    warn!("Connection rejected — max connections reached");
                    drop(stream);
                    continue;
                }
                active.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let auth = Arc::clone(auth);
                let state = Arc::clone(state);
                let inject_dir = Arc::clone(inject_dir);
                let active = Arc::clone(&active);
                std::thread::spawn(move || {
                    if let Err(e) = handle_connection(stream, &auth, &state, &inject_dir) {
                        debug!(error = %e, "Connection closed");
                    }
                    active.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
                });
            },
            Err(e) => {
                error!(error = %e, "Failed to accept connection");
            },
        }
    }
}

#[allow(clippy::needless_pass_by_value)] // TcpStream ownership needed for BufReader
fn handle_connection(
    stream: TcpStream,
    auth: &Auth,
    state: &Arc<RwLock<AgentState>>,
    inject_dir: &Arc<RwLock<Option<std::path::PathBuf>>>,
) -> io::Result<()> {
    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    let peer = stream.peer_addr().ok();
    debug!(?peer, "Connection accepted");

    let limited = (&stream).take(MAX_REQUEST_BYTES);
    let mut reader = BufReader::new(limited);
    let mut writer = &stream;
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(()); // client disconnected
        }

        let response = process_request(line.trim(), auth, state, inject_dir);
        writeln!(writer, "{response}")?;
        writer.flush()?;
    }
}

fn process_request(
    raw: &str,
    auth: &Auth,
    state: &Arc<RwLock<AgentState>>,
    inject_dir: &Arc<RwLock<Option<std::path::PathBuf>>>,
) -> String {
    let req: serde_json::Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return r#"{"ok":false,"error":"invalid JSON"}"#.to_string(),
    };

    let token = req["token"].as_str().unwrap_or("");
    if !auth.validate(token) {
        warn!("Rejected request with invalid token");
        return r#"{"ok":false,"error":"unauthorized"}"#.to_string();
    }

    let command = req["command"].as_str().unwrap_or("");
    if command.is_empty() {
        return r#"{"ok":false,"error":"missing command"}"#.to_string();
    }

    let args = req.get("args").cloned().unwrap_or(serde_json::json!({}));
    router::route(command, &args, state, inject_dir).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tempfile::TempDir;

    fn test_auth(dir: &TempDir) -> (Auth, String) {
        let auth = Auth::init(dir.path().join(".cp"), 1).unwrap();
        let content = std::fs::read_to_string(dir.path().join(".cp")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let token = parsed["token"].as_str().unwrap().to_string();
        (auth, token)
    }

    fn test_state() -> Arc<RwLock<AgentState>> {
        Arc::new(RwLock::new(AgentState {
            worker_pid: None,
            worker_restarts: 0,
            worker_alive: false,
            started_at: Instant::now(),
        }))
    }

    fn no_inject() -> Arc<RwLock<Option<std::path::PathBuf>>> {
        Arc::new(RwLock::new(None))
    }

    #[test]
    fn process_valid_ping() {
        let dir = TempDir::new().unwrap();
        let (auth, token) = test_auth(&dir);
        let state = test_state();
        let req = serde_json::json!({"command": "ping", "token": token}).to_string();
        let resp = process_request(&req, &auth, &state, &no_inject());
        let parsed: serde_json::Value = serde_json::from_str(&resp).unwrap();
        assert_eq!(parsed["ok"], true);
    }

    #[test]
    fn process_invalid_json() {
        let dir = TempDir::new().unwrap();
        let (auth, _) = test_auth(&dir);
        let resp = process_request("not json", &auth, &test_state(), &no_inject());
        assert!(resp.contains("invalid JSON"));
    }

    #[test]
    fn process_wrong_token() {
        let dir = TempDir::new().unwrap();
        let (auth, _) = test_auth(&dir);
        let req = r#"{"command":"ping","token":"wrong"}"#;
        let resp = process_request(req, &auth, &test_state(), &no_inject());
        assert!(resp.contains("unauthorized"));
    }

    #[test]
    fn process_missing_command() {
        let dir = TempDir::new().unwrap();
        let (auth, token) = test_auth(&dir);
        let req = serde_json::json!({"token": token}).to_string();
        let resp = process_request(&req, &auth, &test_state(), &no_inject());
        assert!(resp.contains("missing command"));
    }

    #[test]
    fn process_unknown_command() {
        let dir = TempDir::new().unwrap();
        let (auth, token) = test_auth(&dir);
        let req = serde_json::json!({"command": "nope", "token": token}).to_string();
        let resp = process_request(&req, &auth, &test_state(), &no_inject());
        assert!(resp.contains("unknown command"));
    }

    #[test]
    fn bind_returns_port() {
        let (listener, port) = bind().unwrap();
        assert!(port > 0);
        assert_eq!(listener.local_addr().unwrap().ip().to_string(), "127.0.0.1");
    }

    #[test]
    fn end_to_end_over_tcp() {
        let dir = TempDir::new().unwrap();
        let (listener, port) = bind().unwrap();
        let (auth, token) = test_auth(&dir);
        let auth = Arc::new(auth);
        let state = test_state();

        let serve_auth = Arc::clone(&auth);
        let serve_state = Arc::clone(&state);
        let inject = no_inject();
        let serve_inject = Arc::clone(&inject);
        let handle =
            std::thread::spawn(move || serve(&listener, &serve_auth, &serve_state, &serve_inject));

        // Connect as client
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();

        let req = serde_json::json!({"command": "ping", "token": token}).to_string();
        writeln!(stream, "{req}").unwrap();

        let mut reader = BufReader::new(&stream);
        let mut resp = String::new();
        reader.read_line(&mut resp).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(resp.trim()).unwrap();
        assert_eq!(parsed["ok"], true);

        drop(stream);
        drop(handle); // server thread runs forever, will be cleaned up
    }
}
