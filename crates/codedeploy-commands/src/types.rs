//! Request and response types for the `CodeDeploy` Command Service.
//!
//! Field names use `PascalCase` to match the service's JSON wire format.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Shared types
// ---------------------------------------------------------------------------

/// Diagnostic envelope attached to commands.
///
/// `format` max 64 chars, `payload` max 8192 chars.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Envelope {
    pub format: String,
    pub payload: String,
}

/// A host command instance returned by `PollHostCommand`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HostCommandInstance {
    pub host_command_identifier: String,
    pub host_identifier: String,
    pub deployment_execution_id: String,
    pub command_name: String,
    pub nonce: Option<i64>,
}

/// Command status values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommandStatus {
    Pending,
    InProgress,
    Succeeded,
    Failed,
}

/// Deployment specification containing client metadata envelopes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DeploymentSpecification {
    pub generic_envelope: Option<Envelope>,
    pub variant_id: Option<String>,
    pub variant_envelope: Option<Envelope>,
}

// ---------------------------------------------------------------------------
// PollHostCommand
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PollHostCommandInput {
    pub host_identifier: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PollHostCommandOutput {
    pub host_command: Option<HostCommandInstance>,
}

// ---------------------------------------------------------------------------
// PutHostCommandAcknowledgement
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PutHostCommandAcknowledgementInput {
    pub host_command_identifier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Envelope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PutHostCommandAcknowledgementOutput {
    pub command_status: Option<String>,
}

// ---------------------------------------------------------------------------
// GetDeploymentSpecification
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct GetDeploymentSpecificationInput {
    pub deployment_execution_id: String,
    pub host_identifier: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetDeploymentSpecificationOutput {
    pub deployment_system: Option<String>,
    pub deployment_specification: Option<DeploymentSpecification>,
}

// ---------------------------------------------------------------------------
// PutHostCommandComplete
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PutHostCommandCompleteInput {
    pub host_command_identifier: String,
    pub command_status: CommandStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Envelope>,
}

// ---------------------------------------------------------------------------
// PostHostCommandUpdate
// ---------------------------------------------------------------------------

/// `estimated_completion_time` is a `GenericDateTimestamp` serialized as
/// an ISO-8601 string on the wire.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PostHostCommandUpdateInput {
    pub host_command_identifier: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_completion_time: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Envelope>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub(crate) struct PostHostCommandUpdateOutput {
    pub command_status: Option<String>,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_serializes_pascal_case() {
        let env = Envelope { format: "JSON".into(), payload: "{}".into() };
        let json = serde_json::to_string(&env).unwrap();
        assert!(json.contains("\"Format\""));
        assert!(json.contains("\"Payload\""));
    }

    #[test]
    fn envelope_deserializes_pascal_case() {
        let json = r#"{"Format":"JSON","Payload":"{}"}"#;
        let env: Envelope = serde_json::from_str(json).unwrap();
        assert_eq!(env.format, "JSON");
        assert_eq!(env.payload, "{}");
    }

    #[test]
    fn host_command_deserializes_from_service_json() {
        let json = r#"{
            "HostCommandIdentifier": "cmd-123",
            "HostIdentifier": "arn:aws:ec2:us-east-1:123:instance/i-abc",
            "DeploymentExecutionId": "exec-456",
            "CommandName": "Install",
            "Nonce": 42
        }"#;
        let cmd: HostCommandInstance = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.host_command_identifier, "cmd-123");
        assert_eq!(cmd.command_name, "Install");
        assert_eq!(cmd.nonce, Some(42));
    }

    #[test]
    fn host_command_deserializes_without_nonce() {
        let json = r#"{
            "HostCommandIdentifier": "cmd-1",
            "HostIdentifier": "i-1",
            "DeploymentExecutionId": "e-1",
            "CommandName": "BeforeInstall"
        }"#;
        let cmd: HostCommandInstance = serde_json::from_str(json).unwrap();
        assert!(cmd.nonce.is_none());
    }

    #[test]
    fn poll_output_deserializes_null_command() {
        let json = r#"{"HostCommand": null}"#;
        let out: PollHostCommandOutput = serde_json::from_str(json).unwrap();
        assert!(out.host_command.is_none());
    }

    #[test]
    fn poll_output_deserializes_with_command() {
        let json = r#"{"HostCommand": {
            "HostCommandIdentifier": "cmd-1",
            "HostIdentifier": "i-1",
            "DeploymentExecutionId": "e-1",
            "CommandName": "Install"
        }}"#;
        let out: PollHostCommandOutput = serde_json::from_str(json).unwrap();
        assert!(out.host_command.is_some());
        assert_eq!(out.host_command.unwrap().command_name, "Install");
    }

    #[test]
    fn poll_input_serializes_pascal_case() {
        let input = PollHostCommandInput { host_identifier: "i-123".into() };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"HostIdentifier\""));
    }

    #[test]
    fn ack_input_skips_none_diagnostics() {
        let input = PutHostCommandAcknowledgementInput {
            host_command_identifier: "cmd-1".into(),
            diagnostics: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(!json.contains("Diagnostics"));
    }

    #[test]
    fn ack_input_includes_diagnostics_when_present() {
        let input = PutHostCommandAcknowledgementInput {
            host_command_identifier: "cmd-1".into(),
            diagnostics: Some(Envelope { format: "JSON".into(), payload: "{}".into() }),
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"Diagnostics\""));
    }

    #[test]
    fn get_spec_output_deserializes() {
        let json = r#"{
            "DeploymentSystem": "CodeDeploy",
            "DeploymentSpecification": {
                "GenericEnvelope": {"Format": "JSON", "Payload": "{\"key\":\"val\"}"},
                "VariantId": null,
                "VariantEnvelope": null
            }
        }"#;
        let out: GetDeploymentSpecificationOutput = serde_json::from_str(json).unwrap();
        assert_eq!(out.deployment_system.as_deref(), Some("CodeDeploy"));
        let spec = out.deployment_specification.unwrap();
        assert_eq!(spec.generic_envelope.unwrap().format, "JSON");
        assert!(spec.variant_id.is_none());
    }

    #[test]
    fn complete_input_serializes_status() {
        let input = PutHostCommandCompleteInput {
            host_command_identifier: "cmd-1".into(),
            command_status: CommandStatus::Failed,
            diagnostics: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"CommandStatus\":\"Failed\""));
        assert!(!json.contains("Diagnostics"));
    }

    #[test]
    fn command_status_roundtrips() {
        for status in [
            CommandStatus::Pending,
            CommandStatus::InProgress,
            CommandStatus::Succeeded,
            CommandStatus::Failed,
        ] {
            let json = serde_json::to_string(&status).unwrap();
            let back: CommandStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(back, status);
        }
    }

    #[test]
    fn deployment_specification_all_fields() {
        let json = r#"{
            "GenericEnvelope": {"Format": "JSON", "Payload": "gen"},
            "VariantId": "v1",
            "VariantEnvelope": {"Format": "XML", "Payload": "var"}
        }"#;
        let spec: DeploymentSpecification = serde_json::from_str(json).unwrap();
        assert_eq!(spec.generic_envelope.unwrap().payload, "gen");
        assert_eq!(spec.variant_id.as_deref(), Some("v1"));
        assert_eq!(spec.variant_envelope.unwrap().format, "XML");
    }

    #[test]
    fn update_input_includes_estimated_completion_time() {
        let input = PostHostCommandUpdateInput {
            host_command_identifier: "cmd-1".into(),
            estimated_completion_time: Some("2026-02-19T12:00:00Z".into()),
            diagnostics: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("\"EstimatedCompletionTime\":\"2026-02-19T12:00:00Z\""));
        assert!(!json.contains("Diagnostics"));
    }

    #[test]
    fn update_input_skips_none_estimated_completion_time() {
        let input = PostHostCommandUpdateInput {
            host_command_identifier: "cmd-1".into(),
            estimated_completion_time: None,
            diagnostics: None,
        };
        let json = serde_json::to_string(&input).unwrap();
        assert!(!json.contains("EstimatedCompletionTime"));
    }
}
