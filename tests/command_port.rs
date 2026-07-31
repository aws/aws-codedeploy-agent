//! Integration tests for the command port.
//!
//! Starts a real command port, connects over TCP, and exercises all commands.

use codedeploy_agent::command_port;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

fn start_and_discover() -> (u16, String, std::sync::Arc<std::sync::RwLock<command_port::AgentState>>)
{
    let dir = tempfile::TempDir::new().unwrap();
    let discovery_path = dir.path().join(".command-port");
    let (_handle, state) = command_port::start(&discovery_path).unwrap();

    let content = std::fs::read_to_string(&discovery_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let port = parsed["port"].as_u64().unwrap() as u16;
    let token = parsed["token"].as_str().unwrap().to_string();

    // Leak the TempDir so discovery file stays alive for the test
    std::mem::forget(dir);

    (port, token, state)
}

fn send(port: u16, request: &str) -> serde_json::Value {
    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    writeln!(stream, "{request}").unwrap();

    let mut reader = BufReader::new(&stream);
    let mut resp = String::new();
    reader.read_line(&mut resp).unwrap();
    serde_json::from_str(resp.trim()).unwrap()
}

#[test]
fn ping() {
    let (port, token, _state) = start_and_discover();
    let resp = send(port, &format!(r#"{{"command":"ping","token":"{token}"}}"#));
    assert_eq!(resp["ok"], true);
}

#[test]
fn status_idle() {
    let (port, token, _state) = start_and_discover();
    let resp = send(port, &format!(r#"{{"command":"status","token":"{token}"}}"#));
    assert_eq!(resp["ok"], true);
    assert_eq!(resp["data"]["status"], "worker_down");
    assert!(resp["data"]["uptime_secs"].as_u64().is_some());
}

#[test]
fn status_reflects_state_change() {
    let (port, token, state) = start_and_discover();

    {
        let mut s = state.write().unwrap();
        s.worker_pid = Some(9999);
        s.worker_alive = true;
        s.worker_restarts = 2;
    }

    let resp = send(port, &format!(r#"{{"command":"status","token":"{token}"}}"#));
    assert_eq!(resp["data"]["status"], "running");
    assert_eq!(resp["data"]["worker_pid"], 9999);
    assert_eq!(resp["data"]["worker_restarts"], 2);
}

#[test]
fn unauthorized_without_token() {
    let (port, _token, _state) = start_and_discover();
    let resp = send(port, r#"{"command":"ping","token":"wrong"}"#);
    assert_eq!(resp["ok"], false);
    assert!(resp["error"].as_str().unwrap().contains("unauthorized"));
}

#[test]
fn unknown_command() {
    let (port, token, _state) = start_and_discover();
    let resp = send(port, &format!(r#"{{"command":"bogus","token":"{token}"}}"#));
    assert_eq!(resp["ok"], false);
    assert!(resp["error"].as_str().unwrap().contains("unknown command"));
}

#[test]
fn invalid_json() {
    let (port, _token, _state) = start_and_discover();
    let resp = send(port, "not json at all");
    assert_eq!(resp["ok"], false);
    assert!(resp["error"].as_str().unwrap().contains("invalid JSON"));
}

#[test]
fn multiple_commands_on_same_connection() {
    let (port, token, _state) = start_and_discover();

    let mut stream = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());

    // Send ping
    writeln!(stream, r#"{{"command":"ping","token":"{token}"}}"#).unwrap();
    let mut resp = String::new();
    reader.read_line(&mut resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(resp.trim()).unwrap();
    assert_eq!(parsed["ok"], true);

    // Send status on same connection
    resp.clear();
    writeln!(stream, r#"{{"command":"status","token":"{token}"}}"#).unwrap();
    reader.read_line(&mut resp).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(resp.trim()).unwrap();
    assert_eq!(parsed["data"]["status"], "worker_down");
}

#[test]
fn inject_command_via_tcp() {
    let (_port, _token, _state) = start_and_discover();

    // Read discovery file to find inject dir
    let _dir = tempfile::TempDir::new().unwrap();
    // The inject dir is the parent of the discovery path — which is the leaked TempDir.
    // We can't access it, so let's start a fresh command port with a known dir.
    drop(_state);

    let dir2 = tempfile::TempDir::new().unwrap();
    let discovery = dir2.path().join(".command-port");
    let (_handle2, _state2) = command_port::start(&discovery).unwrap();

    let content = std::fs::read_to_string(&discovery).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    let port2 = parsed["port"].as_u64().unwrap() as u16;
    let token2 = parsed["token"].as_str().unwrap();

    // Spawn fake worker that reads command file and writes response
    let inject_dir = dir2.path().to_path_buf();
    let worker = std::thread::spawn(move || {
        let cmd_path = inject_dir.join(".injected-command.json");
        for _ in 0..40 {
            if cmd_path.exists() {
                let content = std::fs::read_to_string(&cmd_path).unwrap();
                let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
                assert_eq!(parsed["command_name"], "AfterInstall");
                std::fs::remove_file(&cmd_path).unwrap();
                let resp_path = inject_dir.join(".injected-response.json");
                std::fs::write(&resp_path, r#"{"ok":true,"data":"executed"}"#).unwrap();
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("command file never appeared");
    });

    let resp = send(
        port2,
        &format!(
            r#"{{"command":"inject","token":"{token2}","args":{{"host_command_identifier":"cmd-1","deployment_execution_id":"exec-1","command_name":"AfterInstall"}}}}"#
        ),
    );
    assert_eq!(resp["ok"], true);

    worker.join().unwrap();
}
