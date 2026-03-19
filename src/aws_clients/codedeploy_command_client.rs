//! @risk high
//!
//! Wrapper around [`codedeploy_commands::Client`] that adapts the agent's
//! credential model to the low-level HTTP client.
//!
//! Ruby source: `lib/instance_agent/plugins/codedeploy/codedeploy_control.rb`

//! `CodeDeploy` command service client — polls, acks, and completes host commands.
use crate::aws_clients::credentials::{CredentialMode, Credentials};
use aws_credential_types::Credentials as AwsCredentials;
use codedeploy_commands::types::{CommandStatus, Envelope, HostCommandInstance};
use codedeploy_commands::{Client, GetDeploymentSpecificationOutput};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CodeDeployClientError {
    #[error("CodeDeploy command service error: {0}")]
    Service(#[from] codedeploy_commands::Error),

    #[error("SSL verification failed: {0}")]
    SslVerificationFailed(String),

    #[error("Invalid endpoint: {0}")]
    InvalidEndpoint(String),

    #[error("Unsupported credential mode for CodeDeploy client: IamSession")]
    UnsupportedCredentialMode,

    #[error("IMDS credentials unavailable: {0}")]
    ImdsUnavailable(String),
}

/// Host command returned by `PollHostCommand`.
///
/// Thin wrapper so consumers don't depend on the wire-format crate directly.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HostCommand {
    pub host_identifier: String,
    pub host_command_identifier: String,
    pub deployment_execution_id: String,
    pub command_name: String,
}

impl From<HostCommandInstance> for HostCommand {
    fn from(hci: HostCommandInstance) -> Self {
        Self {
            host_identifier: hci.host_identifier,
            host_command_identifier: hci.host_command_identifier,
            deployment_execution_id: hci.deployment_execution_id,
            command_name: hci.command_name,
        }
    }
}

/// Deployment specification returned by `GetDeploymentSpecification`.
#[derive(Debug)]
pub struct DeploymentSpecification {
    pub deployment_system: String,
    pub generic_envelope: String,
    pub envelope_format: String,
}

/// Maps the ACK response status string to a typed enum.
///
/// Ruby: `command_poller.rb` checks `command_status` string from ACK response.
#[derive(Debug, PartialEq, Eq)]
pub enum AckStatus {
    Succeeded,
    Failed,
    /// Any other status — command should proceed.
    InProgress,
}

impl AckStatus {
    /// Parse the status string returned by `PutHostCommandAcknowledgement`.
    ///
    /// Ruby: `command_poller.rb` treats `"Succeeded"` and `"Failed"` as terminal,
    /// everything else (including `None`) as in-progress.
    #[must_use]
    pub fn from_response(status: Option<&str>) -> Self {
        match status {
            Some("Succeeded") => Self::Succeeded,
            Some("Failed") => Self::Failed,
            _ => Self::InProgress,
        }
    }
}

#[derive(Debug)]
pub struct CodeDeployCommandClient {
    inner: Client,
    credentials: Credentials,
}

impl CodeDeployCommandClient {
    /// Create a new `CodeDeploy` command client.
    ///
    /// Converts the agent's [`Credentials`] into AWS SDK credentials and builds
    /// the underlying HTTP client.
    ///
    /// # Errors
    /// Returns error if credential mode is unsupported or client build fails.
    pub fn new(
        credentials: Credentials,
        use_fips: bool,
        enable_auth_policy: bool,
        endpoint_override: Option<String>,
        http_read_timeout: Duration,
        proxy_uri: Option<String>,
    ) -> Result<Self, CodeDeployClientError> {
        let aws_creds = to_aws_credentials(&credentials)?;

        let inner = Client::builder()
            .region(&credentials.region)
            .credentials(aws_creds)
            .use_fips(use_fips)
            .enable_auth_policy(enable_auth_policy)
            .endpoint(endpoint_override)
            .http_read_timeout(http_read_timeout)
            .proxy_uri(proxy_uri)
            .build()
            .map_err(CodeDeployClientError::Service)?;

        Ok(Self { inner, credentials })
    }

    /// Poll for a host command from `CodeDeploy`.
    ///
    /// Ruby: `@deploy_control_client.poll_host_command(host_identifier: @host_identifier)`
    ///
    /// # Errors
    /// Returns error if the HTTP request fails.
    pub fn poll_host_command(
        &self,
        host_identifier: &str,
    ) -> Result<Option<HostCommand>, CodeDeployClientError> {
        let result = self.inner.poll_host_command(host_identifier)?;
        Ok(result.map(HostCommand::from))
    }

    /// Acknowledge receipt of a command.
    ///
    /// Ruby: `@deploy_control_client.put_host_command_acknowledgement(
    ///   host_command_identifier: ..., diagnostics: {format: "JSON", payload: ...})`
    ///
    /// # Errors
    /// Returns error if the HTTP request fails.
    pub fn put_host_command_acknowledgement(
        &self,
        host_command_identifier: &str,
        diagnostics_payload: &str,
    ) -> Result<AckStatus, CodeDeployClientError> {
        let envelope =
            Envelope { format: "JSON".to_string(), payload: diagnostics_payload.to_string() };

        let status_str = self
            .inner
            .put_host_command_acknowledgement(host_command_identifier, Some(&envelope))?;

        Ok(AckStatus::from_response(status_str.as_deref()))
    }

    /// Get deployment specification.
    ///
    /// Ruby: `@deploy_control_client.get_deployment_specification(
    ///   deployment_execution_id: ..., host_identifier: ...)`
    ///
    /// # Errors
    /// Returns error if the HTTP request fails or the response is missing required fields.
    pub fn get_deployment_specification(
        &self,
        deployment_execution_id: &str,
        host_identifier: &str,
    ) -> Result<DeploymentSpecification, CodeDeployClientError> {
        let output = self
            .inner
            .get_deployment_specification(deployment_execution_id, host_identifier)?;

        Ok(to_deployment_spec(output))
    }

    /// Report command completion.
    ///
    /// Ruby: `@deploy_control_client.put_host_command_complete(
    ///   host_command_identifier: ..., command_status: ..., diagnostics: ...)`
    ///
    /// # Errors
    /// Returns error if the HTTP request fails.
    pub fn put_host_command_complete(
        &self,
        host_command_identifier: &str,
        status: &str,
        diagnostics_payload: &str,
    ) -> Result<(), CodeDeployClientError> {
        // Map status string to enum. Unknown statuses default to Failed.
        let command_status = match status {
            "Succeeded" => CommandStatus::Succeeded,
            "InProgress" => CommandStatus::InProgress,
            "Pending" => CommandStatus::Pending,
            // "Failed" and any unknown status default to Failed
            _ => CommandStatus::Failed,
        };

        let envelope =
            Envelope { format: "JSON".to_string(), payload: diagnostics_payload.to_string() };

        self.inner.put_host_command_complete(
            host_command_identifier,
            command_status,
            Some(&envelope),
        )?;

        Ok(())
    }

    /// The configured region.
    #[must_use]
    pub fn region(&self) -> &str {
        self.inner.region()
    }

    /// The agent credentials.
    #[must_use]
    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }
}

/// Convert agent credentials to AWS SDK credentials.
fn to_aws_credentials(creds: &Credentials) -> Result<AwsCredentials, CodeDeployClientError> {
    match &creds.mode {
        CredentialMode::IamUser { access_key_id, secret_access_key } => {
            Ok(AwsCredentials::new(access_key_id, secret_access_key, None, None, "codedeploy"))
        },
        CredentialMode::InstanceProfile => {
            // Ruby: `Aws::InstanceProfileCredentials.new` auto-refreshes when credentials
            // approach expiration (~6h). We fetch a static snapshot here at client creation.
            // The worker recreates the client on fatal polling errors (worker restart),
            // which provides implicit refresh. If the agent runs >6h without errors,
            // credentials will expire. TODO: wire `ImdsCredentialsProvider` as a
            // long-lived credential source instead of a one-shot fetch.
            //
            // Ruby behavior: raises if IMDS is unreachable — agent cannot start without
            // valid credentials. We match this by returning Err on IMDS failure.
            use crate::aws_clients::imds;
            match imds::fetch_credentials() {
                Ok(aws_creds) => {
                    tracing::info!("Loaded credentials from IMDS instance profile");
                    Ok(aws_creds)
                },
                Err(e) => Err(CodeDeployClientError::ImdsUnavailable(e.to_string())),
            }
        },
        CredentialMode::IamSession { .. } => Err(CodeDeployClientError::UnsupportedCredentialMode),
    }
}

/// Convert the crate's output type to our wrapper type.
fn to_deployment_spec(output: GetDeploymentSpecificationOutput) -> DeploymentSpecification {
    let deployment_system = output.deployment_system.unwrap_or_default();
    let (generic_envelope, envelope_format) = output
        .deployment_specification
        .and_then(|spec| spec.generic_envelope)
        .map_or_else(|| (String::new(), String::new()), |env| (env.payload, env.format));

    DeploymentSpecification { deployment_system, generic_envelope, envelope_format }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_clients::credentials::{CredentialMode, Credentials};

    fn instance_profile_credentials() -> Credentials {
        Credentials {
            region: "us-east-1".to_string(),
            host_identifier: "i-1234567890abcdef0".to_string(),
            mode: CredentialMode::InstanceProfile,
        }
    }

    fn iam_user_credentials() -> Credentials {
        Credentials {
            region: "us-west-2".to_string(),
            host_identifier: "arn:aws:iam::123:user/test".to_string(),
            mode: CredentialMode::IamUser {
                access_key_id: "AKIATEST".to_string(),
                secret_access_key: "secret".to_string(),
            },
        }
    }

    fn iam_session_credentials() -> Credentials {
        Credentials {
            region: "us-east-1".to_string(),
            host_identifier: "arn:aws:sts::123:assumed-role/test".to_string(),
            mode: CredentialMode::IamSession { credentials_file: "/path/to/creds".into() },
        }
    }

    #[test]
    fn new_client_with_instance_profile() {
        // On EC2/dev hosts: succeeds with real IMDS credentials.
        // Off EC2: fails with ImdsUnavailable (fail-fast, matches Ruby).
        let result = CodeDeployCommandClient::new(
            instance_profile_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        );
        match result {
            Ok(client) => {
                assert_eq!(client.region(), "us-east-1");
                assert_eq!(client.credentials().host_identifier, "i-1234567890abcdef0");
            },
            Err(e) => {
                assert!(e.to_string().contains("IMDS"), "expected IMDS error but got: {e}");
            },
        }
    }

    #[test]
    fn new_client_with_iam_user() {
        let client = CodeDeployCommandClient::new(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        assert_eq!(client.region(), "us-west-2");
        assert_eq!(client.credentials().host_identifier, "arn:aws:iam::123:user/test");
    }

    #[test]
    fn new_client_with_iam_session_fails() {
        let result = CodeDeployCommandClient::new(
            iam_session_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported"));
    }

    #[test]
    fn new_client_with_fips() {
        // Uses instance profile — may fail off-EC2 (no IMDS).
        let result = CodeDeployCommandClient::new(
            instance_profile_credentials(),
            true,
            false,
            None,
            Duration::from_secs(80),
            None,
        );
        match result {
            Ok(client) => assert_eq!(client.region(), "us-east-1"),
            Err(e) => assert!(e.to_string().contains("IMDS"), "expected IMDS error but got: {e}"),
        }
    }

    #[test]
    fn new_client_with_custom_endpoint() {
        // Uses instance profile — may fail off-EC2 (no IMDS).
        let result = CodeDeployCommandClient::new(
            instance_profile_credentials(),
            false,
            false,
            Some("https://custom.endpoint".to_string()),
            Duration::from_secs(80),
            None,
        );
        match result {
            Ok(client) => assert_eq!(client.region(), "us-east-1"),
            Err(e) => assert!(e.to_string().contains("IMDS"), "expected IMDS error but got: {e}"),
        }
    }

    #[test]
    fn to_aws_credentials_iam_user() {
        let creds = iam_user_credentials();
        let aws_creds = to_aws_credentials(&creds).unwrap();
        assert_eq!(aws_creds.access_key_id(), "AKIATEST");
        assert_eq!(aws_creds.secret_access_key(), "secret");
    }

    #[test]
    fn to_aws_credentials_instance_profile_returns_result() {
        // On EC2/dev hosts with IMDS: returns real credentials from instance profile.
        // Off EC2 (no IMDS): returns ImdsUnavailable error (fail-fast, matches Ruby).
        let creds = instance_profile_credentials();
        let result = to_aws_credentials(&creds);
        match result {
            Ok(aws_creds) => {
                assert!(!aws_creds.access_key_id().is_empty());
                assert!(!aws_creds.secret_access_key().is_empty());
            },
            Err(e) => {
                assert!(e.to_string().contains("IMDS"), "expected IMDS error but got: {e}");
            },
        }
    }

    #[test]
    fn to_aws_credentials_iam_session_unsupported() {
        let creds = iam_session_credentials();
        assert!(to_aws_credentials(&creds).is_err());
    }

    #[test]
    fn to_deployment_spec_full_response() {
        let output = GetDeploymentSpecificationOutput {
            deployment_system: Some("CodeDeploy".to_string()),
            deployment_specification: Some(codedeploy_commands::DeploymentSpecification {
                generic_envelope: Some(Envelope {
                    format: "JSON".to_string(),
                    payload: "{\"key\":\"val\"}".to_string(),
                }),
                variant_id: None,
                variant_envelope: None,
            }),
        };
        let spec = to_deployment_spec(output);
        assert_eq!(spec.deployment_system, "CodeDeploy");
        assert_eq!(spec.generic_envelope, "{\"key\":\"val\"}");
        assert_eq!(spec.envelope_format, "JSON");
    }

    #[test]
    fn to_deployment_spec_missing_fields() {
        let output = GetDeploymentSpecificationOutput {
            deployment_system: None,
            deployment_specification: None,
        };
        let spec = to_deployment_spec(output);
        assert_eq!(spec.deployment_system, "");
        assert_eq!(spec.generic_envelope, "");
        assert_eq!(spec.envelope_format, "");
    }

    #[test]
    fn host_command_from_instance() {
        let hci = HostCommandInstance {
            host_identifier: "i-1".to_string(),
            host_command_identifier: "cmd-1".to_string(),
            deployment_execution_id: "exec-1".to_string(),
            command_name: "Install".to_string(),
            nonce: Some(42),
        };
        let cmd = HostCommand::from(hci);
        assert_eq!(cmd.host_identifier, "i-1");
        assert_eq!(cmd.host_command_identifier, "cmd-1");
        assert_eq!(cmd.deployment_execution_id, "exec-1");
        assert_eq!(cmd.command_name, "Install");
    }

    #[test]
    fn ack_status_from_response_succeeded() {
        assert_eq!(AckStatus::from_response(Some("Succeeded")), AckStatus::Succeeded);
    }

    #[test]
    fn ack_status_from_response_failed() {
        assert_eq!(AckStatus::from_response(Some("Failed")), AckStatus::Failed);
    }

    #[test]
    fn ack_status_from_response_in_progress() {
        assert_eq!(AckStatus::from_response(Some("InProgress")), AckStatus::InProgress);
    }

    #[test]
    fn ack_status_from_response_none_maps_to_in_progress() {
        assert_eq!(AckStatus::from_response(None), AckStatus::InProgress);
    }

    #[test]
    fn ack_status_from_response_unknown_maps_to_in_progress() {
        assert_eq!(AckStatus::from_response(Some("Pending")), AckStatus::InProgress);
        assert_eq!(AckStatus::from_response(Some("")), AckStatus::InProgress);
    }

    #[test]
    fn error_display() {
        let e1 = CodeDeployClientError::SslVerificationFailed("cert expired".to_string());
        assert!(e1.to_string().contains("cert expired"));

        let e2 = CodeDeployClientError::InvalidEndpoint("bad url".to_string());
        assert!(e2.to_string().contains("bad url"));

        let e3 = CodeDeployClientError::UnsupportedCredentialMode;
        assert!(e3.to_string().contains("Unsupported"));

        let e4 = CodeDeployClientError::ImdsUnavailable("connection refused".to_string());
        assert!(e4.to_string().contains("IMDS"));
        assert!(e4.to_string().contains("connection refused"));
    }

    #[test]
    fn types_are_debug() {
        let _ = format!("{:?}", AckStatus::Succeeded);
        let _ = format!("{:?}", AckStatus::Failed);
        let _ = format!("{:?}", AckStatus::InProgress);
        let _ = format!(
            "{:?}",
            HostCommand {
                host_identifier: "h1".to_string(),
                host_command_identifier: "c1".to_string(),
                deployment_execution_id: "d1".to_string(),
                command_name: "Install".to_string(),
            }
        );
        let _ = format!(
            "{:?}",
            DeploymentSpecification {
                deployment_system: "CodeDeploy".to_string(),
                generic_envelope: "{}".to_string(),
                envelope_format: "TEXT/JSON".to_string(),
            }
        );
    }
}
