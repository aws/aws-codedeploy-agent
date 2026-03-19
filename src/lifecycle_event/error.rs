//! @risk none
//!
//! Script execution errors and diagnostics.
//!
//! `ScriptError` carries the error code, script name, log tail, and message
//! needed by the `CodeDeploy` service's `PutHostCommandComplete` API.

use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Succeeded = 0,
    ScriptMissing = 1,
    ScriptExecutability = 2,
    ScriptTimedOut = 3,
    ScriptFailed = 4,
    UnknownError = 5,
    OutputsLeftOpen = 6,
    FailedAfterRestart = 7,
}

#[derive(Debug)]
pub struct ScriptError {
    pub error_code: ErrorCode,
    pub script_name: String,
    pub log: Vec<String>,
    pub message: String,
}

impl ScriptError {
    #[must_use]
    pub fn new(
        error_code: ErrorCode,
        script_name: String,
        log: Vec<String>,
        message: String,
    ) -> Self {
        Self { error_code, script_name, log, message }
    }

    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        // Log entries are joined into a single string for the service
        let log = self.log.join("");
        json!({
            "error_code": self.error_code as i32,
            "script_name": self.script_name,
            "message": self.message,
            "log": log
        })
    }
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.script_name, self.message)
    }
}

impl std::error::Error for ScriptError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_code_values() {
        assert_eq!(ErrorCode::Succeeded as i32, 0);
        assert_eq!(ErrorCode::ScriptMissing as i32, 1);
        assert_eq!(ErrorCode::ScriptExecutability as i32, 2);
        assert_eq!(ErrorCode::ScriptTimedOut as i32, 3);
        assert_eq!(ErrorCode::ScriptFailed as i32, 4);
        assert_eq!(ErrorCode::UnknownError as i32, 5);
        assert_eq!(ErrorCode::OutputsLeftOpen as i32, 6);
        assert_eq!(ErrorCode::FailedAfterRestart as i32, 7);
    }

    #[test]
    fn script_error_new() {
        let err = ScriptError::new(
            ErrorCode::ScriptFailed,
            "scripts/start.sh".into(),
            vec!["line1".into()],
            "exit code 1".into(),
        );
        assert_eq!(err.error_code, ErrorCode::ScriptFailed);
        assert_eq!(err.script_name, "scripts/start.sh");
        assert_eq!(err.log, vec!["line1"]);
        assert_eq!(err.message, "exit code 1");
    }

    #[test]
    fn script_error_display() {
        let err = ScriptError::new(
            ErrorCode::ScriptFailed,
            "start.sh".into(),
            Vec::new(),
            "failed".into(),
        );
        assert_eq!(err.to_string(), "start.sh: failed");
    }

    #[test]
    fn script_error_to_json() {
        let err = ScriptError::new(
            ErrorCode::ScriptTimedOut,
            "deploy.sh".into(),
            vec!["log1".into(), "log2".into()],
            "timed out".into(),
        );
        let json = err.to_json();
        assert_eq!(json["error_code"], 3);
        assert_eq!(json["script_name"], "deploy.sh");
        assert_eq!(json["message"], "timed out");
        assert_eq!(json["log"], "log1log2");
    }

    #[test]
    fn script_error_empty_log_json() {
        let err = ScriptError::new(ErrorCode::Succeeded, String::new(), Vec::new(), "ok".into());
        let json = err.to_json();
        assert_eq!(json["error_code"], 0);
        assert_eq!(json["log"], "");
    }

    #[test]
    fn script_error_is_std_error() {
        let err =
            ScriptError::new(ErrorCode::UnknownError, String::new(), Vec::new(), "msg".into());
        let _: &dyn std::error::Error = &err;
    }
}
