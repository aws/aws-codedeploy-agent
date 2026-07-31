//! Inject command — writes a `HostCommand` to a file for the poller to pick up.

use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Shared path where injected commands are written.
#[derive(Debug)]
pub struct InjectPath(pub PathBuf);

/// Write an injected command file and wait for the poller to process it.
#[must_use]
pub fn handle(args: &Value, inject_dir: &Arc<RwLock<Option<PathBuf>>>) -> Value {
    let dir = inject_dir.read().unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(dir) = dir.as_ref() else {
        return json!({"ok": false, "error": "inject not configured — state_dir not set"});
    };

    // Validate required fields
    let Some(host_command_identifier) = args.get("host_command_identifier").and_then(Value::as_str)
    else {
        return json!({"ok": false, "error": "missing host_command_identifier"});
    };
    let Some(deployment_execution_id) = args.get("deployment_execution_id").and_then(Value::as_str)
    else {
        return json!({"ok": false, "error": "missing deployment_execution_id"});
    };
    let Some(command_name) = args.get("command_name").and_then(Value::as_str) else {
        return json!({"ok": false, "error": "missing command_name"});
    };
    let host_identifier = args.get("host_identifier").and_then(Value::as_str).unwrap_or("local");

    let command = json!({
        "host_identifier": host_identifier,
        "host_command_identifier": host_command_identifier,
        "deployment_execution_id": deployment_execution_id,
        "command_name": command_name,
    });

    // Write command file (atomic via temp + rename)
    let cmd_path = dir.join(".injected-command.json");
    let tmp_path = dir.join(".injected-command.tmp");
    if let Err(e) = write_atomic(&tmp_path, &cmd_path, &command.to_string()) {
        return json!({"ok": false, "error": format!("failed to write command file: {e}")}); // GRCOV_IGNORE_LINE
    }

    // Wait for response file (poller deletes command file and writes response)
    let resp_path = dir.join(".injected-response.json");
    // GRCOV_STOP_COVERAGE — blocks up to 120s waiting for poller
    match wait_for_response(&resp_path, std::time::Duration::from_mins(2)) {
        Ok(resp) => resp,
        Err(e) => json!({"ok": false, "error": format!("timed out waiting for response: {e}")}),
    }
    // GRCOV_BEGIN_COVERAGE
}

fn write_atomic(tmp: &Path, dest: &Path, content: &str) -> std::io::Result<()> {
    std::fs::write(tmp, content)?;
    std::fs::rename(tmp, dest)
}

// GRCOV_STOP_COVERAGE — blocks polling for response file
fn wait_for_response(path: &Path, timeout: std::time::Duration) -> Result<Value, String> {
    let deadline = std::time::Instant::now() + timeout;
    while std::time::Instant::now() < deadline {
        if path.exists() {
            let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            let _ = std::fs::remove_file(path);
            return serde_json::from_str(&content).map_err(|e| e.to_string());
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    Err("no response within timeout".into())
}
// GRCOV_BEGIN_COVERAGE

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn inject_dir(dir: &TempDir) -> Arc<RwLock<Option<PathBuf>>> {
        Arc::new(RwLock::new(Some(dir.path().to_path_buf())))
    }

    #[test]
    fn missing_host_command_identifier() {
        let dir = TempDir::new().unwrap();
        let path = inject_dir(&dir);
        let resp = handle(&json!({}), &path);
        assert!(resp["error"].as_str().unwrap().contains("host_command_identifier"));
    }

    #[test]
    fn missing_deployment_execution_id() {
        let dir = TempDir::new().unwrap();
        let path = inject_dir(&dir);
        let resp = handle(&json!({"host_command_identifier": "c"}), &path);
        assert!(resp["error"].as_str().unwrap().contains("deployment_execution_id"));
    }

    #[test]
    fn missing_command_name() {
        let dir = TempDir::new().unwrap();
        let path = inject_dir(&dir);
        let resp =
            handle(&json!({"host_command_identifier": "c", "deployment_execution_id": "d"}), &path);
        assert!(resp["error"].as_str().unwrap().contains("command_name"));
    }

    #[test]
    fn not_configured() {
        let path = Arc::new(RwLock::new(None));
        let resp = handle(
            &json!({"host_command_identifier": "c", "deployment_execution_id": "d", "command_name": "Install"}),
            &path,
        );
        assert_eq!(resp["ok"], false);
        assert!(resp["error"].as_str().unwrap().contains("not configured"));
    }

    #[test]
    fn writes_command_file() {
        let dir = TempDir::new().unwrap();
        let path = inject_dir(&dir);

        // Spawn a thread that writes a fake response so handle() doesn't block forever
        let resp_path = dir.path().join(".injected-response.json");
        let resp_path_clone = resp_path.clone();
        let cmd_path = dir.path().join(".injected-command.json");
        std::thread::spawn(move || {
            // Wait for command file to appear
            for _ in 0..20 {
                if cmd_path.exists() {
                    // Verify it's valid JSON
                    let content = std::fs::read_to_string(&cmd_path).unwrap();
                    let parsed: Value = serde_json::from_str(&content).unwrap();
                    assert_eq!(parsed["command_name"], "Install");
                    std::fs::remove_file(&cmd_path).unwrap();
                    // Write response
                    std::fs::write(&resp_path_clone, r#"{"ok":true,"data":"done"}"#).unwrap();
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            panic!("command file never appeared");
        });

        let resp = handle(
            &json!({
                "host_command_identifier": "cmd-1",
                "deployment_execution_id": "exec-1",
                "command_name": "Install",
            }),
            &path,
        );
        assert_eq!(resp["ok"], true);
    }

    #[test]
    fn write_atomic_works() {
        let dir = TempDir::new().unwrap();
        let tmp = dir.path().join("tmp");
        let dest = dir.path().join("dest");
        write_atomic(&tmp, &dest, "hello").unwrap();
        assert!(!tmp.exists());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello");
    }
}
