//! Diagnostics formatting for host command results.
//!
//! Formats success/error/script diagnostics as JSON payloads for
//! `put_host_command_complete` and `put_host_command_acknowledgement`.
//! Each payload includes an error code, script name, message, and log tail.

use crate::lifecycle_event::ScriptError;
use serde_json::json;

/// Format a successful completion diagnostic.
#[must_use]
pub fn success(msg: &str) -> String {
    let label = if msg.is_empty() {
        "Succeeded".to_string()
    } else {
        format!("Succeeded: {msg}")
    };
    json!({
        "error_code": 0,
        "script_name": "",
        "message": label,
        "log": ""
    })
    .to_string()
}

/// Format a generic error diagnostic.
#[must_use]
pub fn from_error(err: &dyn std::error::Error) -> String {
    json!({
        "error_code": 5,
        "script_name": "",
        "message": err.to_string(),
        "log": ""
    })
    .to_string()
}

/// Format a script error diagnostic.
/// `to_json` only does in-memory string operations that cannot fail.
#[must_use]
pub fn from_script_error(err: &ScriptError) -> String {
    err.to_json().to_string()
}

/// Format a diagnostic for a deployment that failed after agent restart.
#[must_use]
pub fn from_failure_after_restart(msg: &str) -> String {
    json!({
        "error_code": 7,
        "script_name": "",
        "message": format!("Failed: {msg}"),
        "log": ""
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_empty_message() {
        let json = success("");
        assert!(json.contains("Succeeded"));
        assert!(json.contains("\"error_code\":0"));
    }

    #[test]
    fn success_with_message() {
        let json = success("CompletedNoopCommand");
        assert!(json.contains("Succeeded: CompletedNoopCommand"));
    }

    #[test]
    fn from_error_formats_message() {
        let err = std::io::Error::other("something broke");
        let json = from_error(&err);
        assert!(json.contains("something broke"));
        assert!(json.contains("\"error_code\":5"));
    }

    #[test]
    fn from_script_error_formats() {
        use crate::lifecycle_event::{ErrorCode, ScriptError};
        let err = ScriptError::new(
            ErrorCode::ScriptFailed,
            "deploy.sh".into(),
            Vec::new(),
            "exit 1".into(),
        );
        let json = from_script_error(&err);
        assert!(json.contains("deploy.sh"));
    }

    #[test]
    fn from_failure_after_restart_formats() {
        let json = from_failure_after_restart("agent restarted");
        assert!(json.contains("Failed: agent restarted"));
        assert!(json.contains("\"error_code\":7"));
    }
}
