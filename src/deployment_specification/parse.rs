//! @risk high
//!
//! Deployment spec JSON parsing with token redaction.
use super::builder;
use super::envelope;
use super::error::{DeploymentSpecError, Result};
use super::revision;
use super::types::{DeploymentSpec, Envelope};
use crate::system::{EnvOps, SystemEnvOps};
use serde_json::Value;
use tracing::debug;

impl DeploymentSpec {
    /// # Errors
    /// Returns an error if the JSON data is missing required fields or contains invalid values.
    pub fn new(data: &Value) -> Result<Self> {
        let (revision_source, revision) = revision::parse(data)?;
        builder::build(data, revision_source, revision)
    }

    /// # Errors
    /// Returns an error if signature verification fails or the envelope contains invalid data.
    pub fn parse(envelope: &Envelope) -> Result<Self> {
        Self::parse_with_env(envelope, &SystemEnvOps)
    }

    /// Parse with injectable env ops (for testing).
    ///
    /// # Errors
    /// Returns an error if signature verification fails or the envelope contains invalid data.
    pub fn parse_with_env(envelope: &Envelope, env: &dyn EnvOps) -> Result<Self> {
        let json_data = envelope::verify_and_extract(envelope, env)?;
        parse_deployment_spec_data(&json_data)
    }
}

fn parse_deployment_spec_data(data: &str) -> Result<DeploymentSpec> {
    let mut parsed: Value = serde_json::from_str(data)
        .map_err(|e| DeploymentSpecError::ParseError(format!("JSON parse error: {e}")))?;

    // @risk critical — token must be redacted before logging
    if let Some(token) = parsed.get_mut("GitHubAccessToken") {
        *token = Value::String("REDACTED".to_string());
    }

    debug!("Parse: {parsed}");

    DeploymentSpec::new(&parsed)
}

#[cfg(test)]
mod tests {
    use crate::deployment_specification::types::RevisionLocation;
    use crate::deployment_specification::{DeploymentSpec, Envelope};
    use crate::system::MockEnvOps;
    use serde_json::json;

    fn dev_mode_env() -> MockEnvOps {
        MockEnvOps::with("CODEDEPLOY_DEVELOPER_MODE", "true")
    }

    // Integration tests - end-to-end parsing through all modules

    #[test]
    fn parse_s3_revision_integration() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyDeploymentGroup",
            "ApplicationName": "MyApp",
            "Revision": {
                "RevisionType": "S3",
                "S3Revision": {
                    "Bucket": "my-bucket",
                    "Key": "my-key.tar.gz",
                    "BundleType": "tgz",
                    "Version": "v1",
                    "ETag": "abc123"
                }
            }
        });

        let spec = DeploymentSpec::new(&data).unwrap();
        assert_eq!(spec.deployment_id, "d-12345678");
        assert_eq!(spec.application_name, "MyApp");

        assert_eq!(
            spec.revision,
            RevisionLocation::S3 {
                bucket: "my-bucket".to_string(),
                key: "my-key.tar.gz".to_string(),
                bundle_type: "tgz".to_string(),
                version: Some("v1".to_string()),
                etag: Some("abc123".to_string()),
            }
        );
    }

    #[test]
    fn parse_github_revision_integration() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyDeploymentGroup",
            "ApplicationName": "MyApp",
            "Revision": {
                "RevisionType": "GitHub",
                "GitHubRevision": {
                    "Account": "myaccount",
                    "Repository": "myrepo",
                    "CommitId": "abc123def456"
                }
            },
            "GitHubAccessToken": "secret-token"
        });

        let spec = DeploymentSpec::new(&data).unwrap();

        assert_eq!(
            spec.revision,
            RevisionLocation::GitHub {
                account: "myaccount".to_string(),
                repository: "myrepo".to_string(),
                commit_id: "abc123def456".to_string(),
                anonymous: false,
                auth_token: Some("secret-token".to_string()),
                bundle_type: None,
            }
        );
    }

    #[test]
    fn parse_local_revision_integration() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyDeploymentGroup",
            "ApplicationName": "MyApp",
            "Revision": {
                "RevisionType": "Local File",
                "LocalRevision": {
                    "Location": "/tmp/myapp.tar.gz",
                    "BundleType": "tar"
                }
            }
        });

        let spec = DeploymentSpec::new(&data).unwrap();

        assert_eq!(
            spec.revision,
            RevisionLocation::Local {
                location: "/tmp/myapp.tar.gz".to_string(),
                bundle_type: "tar".to_string(),
            }
        );
    }

    #[test]
    fn envelope_text_json_integration() {
        let json_data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyDeploymentGroup",
            "ApplicationName": "MyApp",
            "Revision": {
                "RevisionType": "S3",
                "S3Revision": {
                    "Bucket": "my-bucket",
                    "Key": "my-key.tar.gz",
                    "BundleType": "tar"
                }
            }
        });

        let envelope = Envelope {
            format: "TEXT/JSON".to_string(),
            payload: serde_json::to_string(&json_data).unwrap(),
        };

        let spec = DeploymentSpec::parse_with_env(&envelope, &dev_mode_env()).unwrap();
        assert_eq!(spec.deployment_id, "d-12345678");
    }

    #[test]
    fn github_token_redaction() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyDeploymentGroup",
            "ApplicationName": "MyApp",
            "GitHubAccessToken": "secret-token",
            "Revision": {
                "RevisionType": "GitHub",
                "GitHubRevision": {
                    "Account": "123456789012",
                    "Repository": "owner/repo",
                    "CommitId": "abc123"
                }
            }
        });

        let envelope = Envelope {
            format: "TEXT/JSON".to_string(),
            payload: serde_json::to_string(&data).unwrap(),
        };

        let _spec = DeploymentSpec::parse_with_env(&envelope, &dev_mode_env()).unwrap();
    }
}
