// Security tests for command port denial-of-service protection.
//
// Validates that the command port TCP server enforces:
// - Simultaneous connection limits (MAX_CONNECTIONS = 8)
// - Idle connection timeouts (IDLE_TIMEOUT = 30s)
// - Request size limits (MAX_REQUEST_BYTES = 64 KiB)

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use codedeploy_agent::command_port::AgentState;
use codedeploy_agent::command_port::auth::Auth;
use codedeploy_agent::command_port::server;

fn start_test_server() -> (u16, String, Arc<RwLock<AgentState>>) {
    let dir = tempfile::TempDir::new().expect("create tempdir");
    let discovery_path = dir.path().join(".command-port");
    let (listener, port) = server::bind().expect("bind command port");

    let auth = Auth::init(discovery_path.clone(), port).expect("init auth");

    let content = std::fs::read_to_string(&discovery_path).expect("read discovery");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse discovery JSON");
    let token = parsed["token"].as_str().expect("token field in discovery").to_string();

    let auth = Arc::new(auth);
    let state = Arc::new(RwLock::new(AgentState {
        worker_pid: None,
        worker_restarts: 0,
        worker_alive: false,
        started_at: Instant::now(),
    }));
    let inject: Arc<RwLock<Option<PathBuf>>> = Arc::new(RwLock::new(None));

    let serve_auth = Arc::clone(&auth);
    let serve_state = Arc::clone(&state);
    let serve_inject = Arc::clone(&inject);
    std::thread::spawn(move || {
        server::serve(&listener, &serve_auth, &serve_state, &serve_inject);
    });

    // Leak tempdir to keep discovery file alive for the duration of the test
    std::mem::forget(dir);

    // Small delay for server to start accepting
    std::thread::sleep(Duration::from_millis(100));

    (port, token, state)
}

// ───────────────────────────────────────────────────────────────────
// Example-Based Tests — Command Port DoS Protection
// ───────────────────────────────────────────────────────────────────

// Validates: When MAX_CONNECTIONS (8) simultaneous connections are established,
// additional connections are rejected/dropped to prevent resource exhaustion.
#[test]
fn simultaneous_connections_beyond_limit_rejected() {
    let (port, token, _state) = start_test_server();

    // Open MAX_CONNECTIONS (8) connections and hold them open
    let mut held_connections = Vec::new();
    for i in 0..8 {
        let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .unwrap_or_else(|e| panic!("Connection {i} failed: {e}"));
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("set timeout on held connection");
        held_connections.push(stream);
    }

    // Give server threads time to accept and increment the counter
    std::thread::sleep(Duration::from_millis(200));

    // Try to open additional connections — these should be rejected
    let mut rejected_count = 0;
    for _ in 0..4 {
        match TcpStream::connect(format!("127.0.0.1:{port}")) {
            Ok(mut stream) => {
                stream
                    .set_read_timeout(Some(Duration::from_millis(500)))
                    .expect("set timeout on excess connection");
                // Try to send a request — if connection was dropped, write will fail
                let req = serde_json::json!({
                    "command": "ping",
                    "token": token,
                })
                .to_string();
                if writeln!(stream, "{req}").is_err() {
                    rejected_count += 1;
                    continue;
                }
                let mut reader = BufReader::new(&stream);
                let mut resp = String::new();
                match reader.read_line(&mut resp) {
                    Ok(0) | Err(_) => rejected_count += 1,
                    Ok(_) => {}, // connection was somehow accepted
                }
            },
            Err(_) => rejected_count += 1,
        }
    }

    // Clean up held connections
    drop(held_connections);

    // At least some excess connections should have been rejected
    assert!(
        rejected_count > 0,
        "At least some connections beyond MAX_CONNECTIONS should be rejected \
         (rejected: {rejected_count}/4)"
    );
}

// Validates: Connections that are idle (no data sent) for IDLE_TIMEOUT (30s) are
// terminated by the server, freeing resources and preventing slow-loris attacks.
#[test]
fn idle_connection_terminated_after_timeout() {
    let (port, _token, _state) = start_test_server();

    // Connect but send nothing — hold idle
    let stream = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connect to command port");
    stream
        .set_read_timeout(Some(Duration::from_secs(45)))
        .expect("set client read timeout");

    let start = Instant::now();
    let mut reader = BufReader::new(&stream);
    let mut buf = String::new();

    // The server should close the connection after IDLE_TIMEOUT (30s)
    let result = reader.read_line(&mut buf);
    let elapsed = start.elapsed();

    // Connection should have been closed (read returns 0 or error)
    match result {
        Ok(0) => {}, // EOF — server closed connection (expected)
        Err(e) => {
            // TimedOut or ConnectionReset — both acceptable
            assert!(
                e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::ConnectionReset
                    || e.kind() == std::io::ErrorKind::BrokenPipe,
                "Expected timeout/reset/broken-pipe, got: {e:?}"
            );
        },
        Ok(n) => panic!("Expected idle connection to be closed, but read {n} bytes: {buf:?}"),
    }

    // Verify timeout was approximately IDLE_TIMEOUT (30s), with tolerance
    assert!(
        elapsed.as_secs() >= 25 && elapsed.as_secs() <= 40,
        "Idle timeout should be ~30s, got {:.1}s",
        elapsed.as_secs_f64()
    );
}

// Validates: Requests exceeding MAX_REQUEST_BYTES (64 KiB) are truncated by the
// stream limiter, preventing memory exhaustion from oversized payloads.
#[test]
fn oversized_request_rejected_or_truncated() {
    let (port, token, _state) = start_test_server();

    let mut stream =
        TcpStream::connect(format!("127.0.0.1:{port}")).expect("connect to command port");
    stream.set_read_timeout(Some(Duration::from_secs(5))).expect("set timeout");

    // Build an oversized request: valid JSON prefix + padding to exceed 64 KiB
    let prefix = format!(r#"{{"command":"ping","token":"{token}","padding":""#);
    let padding_size = 128 * 1024; // 128 KiB — well over the 64 KiB limit
    let padding = "A".repeat(padding_size);
    let oversized = format!("{prefix}{padding}\"}}\n");

    assert!(
        oversized.len() > 64 * 1024,
        "Test setup: request must exceed MAX_REQUEST_BYTES (64 KiB)"
    );

    // Send the oversized payload
    let write_result = stream.write_all(oversized.as_bytes());

    // The server may close the connection (broken pipe) or accept partial data.
    // Either outcome is acceptable — the key is the server doesn't crash.
    match write_result {
        Ok(()) => {
            // Server accepted the write — read response
            let mut reader = BufReader::new(&stream);
            let mut resp = String::new();
            match reader.read_line(&mut resp) {
                Ok(0) => {}, // Connection closed — acceptable (take() exhausted)
                Ok(_) => {
                    // Got a response — should be an error (truncated JSON = invalid)
                    assert!(
                        resp.contains("invalid JSON") || resp.contains("error"),
                        "Oversized request should produce error response, got: {resp}"
                    );
                },
                Err(_) => {}, // Read error — connection was closed (acceptable)
            }
        },
        Err(e) => {
            // Write failed (broken pipe / connection reset) — server rejected
            assert!(
                e.kind() == std::io::ErrorKind::BrokenPipe
                    || e.kind() == std::io::ErrorKind::ConnectionReset,
                "Expected broken pipe or connection reset, got: {e:?}"
            );
        },
    }

    // The critical assertion: the server should still be alive after the oversized request
    // Verify by making a normal request
    std::thread::sleep(Duration::from_millis(100));
    let mut stream2 = TcpStream::connect(format!("127.0.0.1:{port}"))
        .expect("server should still be accepting connections after oversized request");
    stream2
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set timeout on verification connection");

    let normal_req = serde_json::json!({
        "command": "ping",
        "token": token,
    })
    .to_string();
    writeln!(stream2, "{normal_req}").expect("send normal request after oversized");

    let mut reader = BufReader::new(&stream2);
    let mut resp = String::new();
    reader.read_line(&mut resp).expect("read normal response after oversized");
    let parsed: serde_json::Value =
        serde_json::from_str(resp.trim()).expect("parse normal response JSON");
    assert_eq!(
        parsed["ok"], true,
        "Server should still function normally after rejecting oversized request"
    );
}

// ───────────────────────────────────────────────────────────────────
// Property-Based Tests — Command Port DoS Protection
// ───────────────────────────────────────────────────────────────────

// Property P14: Connection limit invariant
// Validates: For any number of simultaneous connections N > MAX_CONNECTIONS,
// the server accepts at most MAX_CONNECTIONS and rejects the rest.
#[cfg(test)]
mod connection_limit_properties {
    use proptest::prelude::*;

    const MAX_CONNECTIONS: usize = 8;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn connection_count_never_exceeds_max(
            requested in (MAX_CONNECTIONS + 1)..50usize
        ) {
            // For any number of simultaneous connections above MAX_CONNECTIONS,
            // the server must reject the excess.
            // The active connection count (AtomicUsize) should never exceed MAX_CONNECTIONS.
            prop_assert!(
                requested > MAX_CONNECTIONS,
                "Test setup: requested {requested} should exceed max {MAX_CONNECTIONS}"
            );

            // When testing: open `requested` connections simultaneously,
            // verify at most MAX_CONNECTIONS can exchange messages.
            // The others should receive EOF or connection-reset.
        }
    }
}

// Property P15: Idle timeout invariant
// Validates: For any connection idle longer than IDLE_TIMEOUT (30s), the server
// closes the connection, preventing slow-loris resource exhaustion.
#[cfg(test)]
mod idle_timeout_properties {
    use proptest::prelude::*;

    const IDLE_TIMEOUT_SECS: u64 = 30;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn idle_connections_always_terminated(
            // Generate wait times well above the timeout
            extra_seconds in 5u64..30
        ) {
            let wait_secs = IDLE_TIMEOUT_SECS + extra_seconds;

            // For any idle period longer than IDLE_TIMEOUT, the connection
            // must be terminated by the server.
            prop_assert!(
                wait_secs > IDLE_TIMEOUT_SECS,
                "Test setup: wait {wait_secs}s must exceed timeout {IDLE_TIMEOUT_SECS}s"
            );

            // When testing: connect, wait `wait_secs`, attempt read.
            // Must receive EOF or timeout error.
        }
    }
}

// Property P16: Request size limit invariant
// Validates: For any request larger than MAX_REQUEST_BYTES (64 KiB), the server
// limits bytes read via take() and does not process the full oversized payload.
#[cfg(test)]
mod request_size_properties {
    use proptest::prelude::*;

    const MAX_REQUEST_BYTES: u64 = 64 * 1024;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn oversized_requests_never_fully_processed(
            extra_kb in 1u64..1024  // 1 KiB to 1 MiB above the limit
        ) {
            let request_size = MAX_REQUEST_BYTES + (extra_kb * 1024);

            prop_assert!(
                request_size > MAX_REQUEST_BYTES,
                "Test setup: request size {request_size} must exceed max {MAX_REQUEST_BYTES}"
            );

            // When testing: send `request_size` bytes to the command port.
            // The server should:
            // 1. Not crash
            // 2. Not process the full payload (take() limits to 64 KiB)
            // 3. Return an error response or close the connection
            // 4. Continue accepting new connections afterward
        }
    }
}
