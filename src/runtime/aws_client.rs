//! @risk low
//!
//! Runtime AWS client initialization.
use serde::{Deserialize, Serialize};

use super::error::RuntimeError;

/// AWS `CodeDeploy` command from polling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsCommand {
    pub deployment_id: String,
    pub command_id: String,
    pub command_type: String,
    pub spec_data: String,
}

/// Response to AWS `CodeDeploy` service
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AwsResponse {
    pub deployment_id: String,
    pub command_id: String,
    pub status: String,
    pub message: Option<String>,
}

/// Trait for interacting with AWS `CodeDeploy` service
pub trait AwsClient: Send + Sync {
    /// Poll for a new command from AWS `CodeDeploy`
    /// # Errors
    /// Returns an error if polling fails.
    fn poll_command(&self) -> Result<Option<AwsCommand>, RuntimeError>;

    /// Acknowledge receipt of a command
    ///
    /// # Errors
    /// Returns an error if acknowledgment fails.
    fn acknowledge(&self, deployment_id: &str, command_id: &str) -> Result<(), RuntimeError>;

    /// Report successful completion
    ///
    /// # Errors
    /// Returns an error if reporting success fails.
    fn report_success(&self, response: &AwsResponse) -> Result<(), RuntimeError>;

    /// Report failure
    ///
    /// # Errors
    /// Returns an error if reporting failure fails.
    fn report_failure(&self, response: &AwsResponse) -> Result<(), RuntimeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_command_is_cloneable() {
        let cmd = AwsCommand {
            deployment_id: "d-123".into(),
            command_id: "cmd-1".into(),
            command_type: "Install".into(),
            spec_data: "{}".into(),
        };
        let cloned = cmd.clone();
        assert_eq!(cloned.deployment_id, "d-123");
        assert_eq!(cloned.command_id, "cmd-1");
        assert_eq!(cloned.command_type, "Install");
        assert_eq!(cloned.spec_data, "{}");
    }

    #[test]
    fn aws_command_is_debuggable() {
        let cmd = AwsCommand {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            command_type: "Hook".into(),
            spec_data: "data".into(),
        };
        let debug = format!("{cmd:?}");
        assert!(debug.contains("d-1"));
    }

    #[test]
    fn aws_command_serializes_to_json() {
        let cmd = AwsCommand {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            command_type: "Install".into(),
            spec_data: "{}".into(),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains("d-1"));
        assert!(json.contains("Install"));
    }

    #[test]
    fn aws_command_deserializes_from_json() {
        let json =
            r#"{"deployment_id":"d-1","command_id":"c-1","command_type":"Hook","spec_data":"{}"}"#;
        let cmd: AwsCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.deployment_id, "d-1");
        assert_eq!(cmd.command_type, "Hook");
    }

    #[test]
    fn aws_response_is_cloneable() {
        let resp = AwsResponse {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            status: "Succeeded".into(),
            message: Some("done".into()),
        };
        let cloned = resp.clone();
        assert_eq!(cloned.status, "Succeeded");
        assert_eq!(cloned.message, Some("done".into()));
    }

    #[test]
    fn aws_response_none_message() {
        let resp = AwsResponse {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            status: "Failed".into(),
            message: None,
        };
        assert!(resp.message.is_none());
    }

    #[test]
    fn aws_response_serializes_to_json() {
        let resp = AwsResponse {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            status: "Succeeded".into(),
            message: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("Succeeded"));
        assert!(json.contains("null"));
    }

    #[test]
    fn aws_response_deserializes_from_json() {
        let json =
            r#"{"deployment_id":"d-1","command_id":"c-1","status":"Failed","message":"oops"}"#;
        let resp: AwsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.status, "Failed");
        assert_eq!(resp.message, Some("oops".into()));
    }
}
