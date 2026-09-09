//! Wrapper around [`codedeploy_commands::Client`] that adapts the agent's
//! credential model to the low-level HTTP client.

//! `CodeDeploy` command service client — polls, acks, and completes host commands.
use crate::aws_clients::credentials::{
    CREDENTIAL_EXPIRATION, CREDENTIAL_REFRESH_BUFFER, CredentialMode, Credentials,
};
use crate::aws_clients::throttle_gate::ThrottleGate;
use aws_credential_types::Credentials as AwsCredentials;
use codedeploy_commands::types::{CommandStatus, Envelope, HostCommandInstance};
use codedeploy_commands::{Client, GetDeploymentSpecificationOutput};
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;

/// Agent version for the `x-amz-codedeploy-agent-version` header
use crate::system::version_file::AGENT_VERSION;

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

    #[error("Failed to load file credentials: {0}")]
    FileCredentialsError(String),

    #[error("IMDS credentials unavailable: {0}")]
    ImdsUnavailable(String),

    #[error("Internal lock error: {0}")]
    LockPoisoned(String),
}

impl CodeDeployClientError {
    /// Returns `true` if this error is a throttling (HTTP 429) response.
    #[must_use]
    pub fn is_throttle(&self) -> bool {
        matches!(self, Self::Service(e) if *e.kind() == codedeploy_commands::ErrorKind::Throttled)
    }
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
    /// `"Succeeded"` and `"Failed"` are terminal; everything else (including
    /// `None`) is treated as in-progress.
    #[must_use]
    pub fn from_response(status: Option<&str>) -> Self {
        match status {
            Some("Succeeded") => Self::Succeeded,
            Some("Failed") => Self::Failed,
            _ => Self::InProgress,
        }
    }
}

/// Mutable state that gets swapped during credential refresh.
/// Behind an `RwLock` so HTTP methods can hold a read lock (concurrent)
/// while `refresh_if_needed` briefly takes a write lock (~once per 25 min).
struct RefreshableInner {
    client: Client,
    credential_expiry: Option<std::time::Instant>,
}

impl std::fmt::Debug for RefreshableInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefreshableInner")
            .field("credential_expiry", &self.credential_expiry)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct CodeDeployCommandClient {
    /// Read-locked for HTTP calls, write-locked only during credential refresh.
    inner: std::sync::RwLock<RefreshableInner>,
    credentials: Credentials,
    // Builder params stored for client rebuild on credential refresh
    use_fips: bool,
    enable_auth_policy: bool,
    endpoint_override: Option<String>,
    http_read_timeout: Duration,
    proxy_uri: Option<String>,
    /// Process-wide throttle circuit breaker shared across all client instances.
    throttle_gate: Arc<ThrottleGate>,
}

/// Compute the credential expiry instant for the given mode.
fn credential_expiry_for_mode(
    mode: &CredentialMode,
    imds_expiry: Option<std::time::Instant>,
) -> Option<std::time::Instant> {
    match mode {
        CredentialMode::IamSession { .. } => {
            Some(std::time::Instant::now() + CREDENTIAL_EXPIRATION)
        },
        CredentialMode::InstanceProfile => {
            // Minimum cooldown to avoid hammering IMDS when credentials
            // are persistently expired or have no expiry field.
            let floor =
                std::time::Instant::now() + CREDENTIAL_REFRESH_BUFFER + Duration::from_mins(1);
            Some(imds_expiry.map_or(floor, |exp| exp.max(floor)))
        },
        CredentialMode::IamUser { .. } => None,
    }
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
        Self::with_throttle_gate(
            credentials,
            use_fips,
            enable_auth_policy,
            endpoint_override,
            http_read_timeout,
            proxy_uri,
            Arc::new(ThrottleGate::new()),
        )
    }

    /// Create a new client with a shared throttle gate.
    ///
    /// Use this when multiple client instances should share rate-limit state
    /// (e.g., the poller client and the processor client in the same worker).
    ///
    /// # Errors
    /// Returns error if credential mode is unsupported or client build fails.
    pub fn with_throttle_gate(
        credentials: Credentials,
        use_fips: bool,
        enable_auth_policy: bool,
        endpoint_override: Option<String>,
        http_read_timeout: Duration,
        proxy_uri: Option<String>,
        throttle_gate: Arc<ThrottleGate>,
    ) -> Result<Self, CodeDeployClientError> {
        let aws_creds = to_aws_credentials(&credentials)?;
        let imds_expiry = aws_creds.expiry().and_then(system_time_to_instant);

        let inner = Client::builder()
            .region(&credentials.region)
            .credentials(aws_creds)
            .use_fips(use_fips)
            .enable_auth_policy(enable_auth_policy)
            .endpoint(endpoint_override.clone())
            .http_read_timeout(http_read_timeout)
            .proxy_uri(proxy_uri.clone())
            .agent_version(Some(AGENT_VERSION.to_string()))
            .build()
            .map_err(CodeDeployClientError::Service)?;

        let credential_expiry = credential_expiry_for_mode(&credentials.mode, imds_expiry);

        Ok(Self {
            inner: std::sync::RwLock::new(RefreshableInner { client: inner, credential_expiry }),
            credentials,
            use_fips,
            enable_auth_policy,
            endpoint_override,
            http_read_timeout,
            proxy_uri,
            throttle_gate,
        })
    }

    /// Access the shared throttle gate (e.g., for the poll loop to check `is_throttled()`).
    #[must_use]
    pub fn throttle_gate(&self) -> &Arc<ThrottleGate> {
        &self.throttle_gate
    }

    /// Refresh credentials if they are near expiry.
    ///
    /// Rebuilds the inner HTTP client with fresh credentials. For `IamSession`,
    /// re-reads the credentials file. For `InstanceProfile`, re-fetches from IMDS.
    /// No-op for `IamUser` mode (static credentials that don't expire).
    ///
    /// # Errors
    /// Returns error if credentials cannot be loaded or client cannot be rebuilt.
    pub fn refresh_if_needed(&self) -> Result<(), CodeDeployClientError> {
        // Fast path: read lock to check expiry
        {
            let state = self
                .inner
                .read()
                .map_err(|e| CodeDeployClientError::LockPoisoned(format!("{e}")))?;
            let Some(expiry) = state.credential_expiry else {
                return Ok(());
            };
            let now = std::time::Instant::now();
            if now + CREDENTIAL_REFRESH_BUFFER < expiry {
                return Ok(());
            }
        }
        // Attempt refresh; tolerate failure if credentials haven't actually expired.
        match self.try_refresh() {
            Ok(()) => Ok(()),
            Err(e) => {
                let state = self
                    .inner
                    .read()
                    .map_err(|e| CodeDeployClientError::LockPoisoned(format!("{e}")))?;
                if state.credential_expiry.is_some_and(|exp| std::time::Instant::now() < exp) {
                    tracing::warn!(error = %e, "Credential refresh failed, using existing credentials");
                    Ok(())
                } else {
                    Err(e)
                }
            },
        }
    }

    /// Attempt to refresh credentials. Returns error if fetch or client build fails.
    fn try_refresh(&self) -> Result<(), CodeDeployClientError> {
        let mode_name = match &self.credentials.mode {
            CredentialMode::IamUser { .. } => "IamUser",
            CredentialMode::IamSession { .. } => "IamSession",
            CredentialMode::InstanceProfile => "InstanceProfile",
        };
        tracing::info!(mode = mode_name, "Refreshing credentials (near expiry)");
        let aws_creds = to_aws_credentials(&self.credentials)?;
        let imds_expiry = aws_creds.expiry().and_then(system_time_to_instant);
        let new_client = Client::builder()
            .region(&self.credentials.region)
            .credentials(aws_creds)
            .use_fips(self.use_fips)
            .enable_auth_policy(self.enable_auth_policy)
            .endpoint(self.endpoint_override.clone())
            .http_read_timeout(self.http_read_timeout)
            .proxy_uri(self.proxy_uri.clone())
            .agent_version(Some(AGENT_VERSION.to_string()))
            .build()
            .map_err(CodeDeployClientError::Service)?;
        let mut state = self
            .inner
            .write()
            .map_err(|e| CodeDeployClientError::LockPoisoned(format!("{e}")))?;
        // Double-check under write lock — another thread may have refreshed already.
        if let Some(expiry) = state.credential_expiry
            && std::time::Instant::now() + CREDENTIAL_REFRESH_BUFFER < expiry
        {
            return Ok(());
        }
        state.client = new_client;
        state.credential_expiry = credential_expiry_for_mode(&self.credentials.mode, imds_expiry);
        Ok(())
    }

    /// Maximum retry attempts for throttled requests.
    const MAX_THROTTLE_RETRIES: u32 = 5;

    /// Retry a request if it's throttled. On throttle:
    /// 1. Trip the shared gate (all threads back off together)
    /// 2. Wait for the gate to open
    /// 3. Retry
    ///
    /// On success after a previous throttle: reset the gate (half-open → closed).
    /// Non-throttle errors pass through immediately.
    fn retry_on_throttle<T, F>(&self, mut f: F) -> Result<T, CodeDeployClientError>
    where
        F: FnMut() -> Result<T, CodeDeployClientError>,
    {
        self.throttle_gate.wait_if_throttled();

        for attempt in 0..=Self::MAX_THROTTLE_RETRIES {
            match f() {
                Ok(val) => {
                    self.throttle_gate.reset();
                    return Ok(val);
                },
                Err(e) if e.is_throttle() && attempt < Self::MAX_THROTTLE_RETRIES => {
                    tracing::warn!(
                        attempt = attempt + 1,
                        max = Self::MAX_THROTTLE_RETRIES,
                        "Request throttled, tripping gate and retrying"
                    );
                    self.throttle_gate.trip();
                    self.throttle_gate.wait_if_throttled();
                },
                Err(e) => return Err(e),
            }
        }
        unreachable!()
    }

    /// Poll for a host command from `CodeDeploy`.
    ///
    /// # Errors
    /// Returns error if the HTTP request fails.
    pub fn poll_host_command(
        &self,
        host_identifier: &str,
    ) -> Result<Option<HostCommand>, CodeDeployClientError> {
        self.retry_on_throttle(|| {
            self.refresh_if_needed()?;
            let state = self
                .inner
                .read()
                .map_err(|e| CodeDeployClientError::LockPoisoned(format!("{e}")))?;
            let result = state.client.poll_host_command(host_identifier)?;
            Ok(result.map(HostCommand::from))
        })
    }

    /// Acknowledge receipt of a command.
    ///
    /// # Errors
    /// Returns error if the HTTP request fails.
    pub fn put_host_command_acknowledgement(
        &self,
        host_command_identifier: &str,
        diagnostics_payload: &str,
    ) -> Result<AckStatus, CodeDeployClientError> {
        self.retry_on_throttle(|| {
            self.refresh_if_needed()?;
            let envelope =
                Envelope { format: "JSON".to_string(), payload: diagnostics_payload.to_string() };

            let state = self
                .inner
                .read()
                .map_err(|e| CodeDeployClientError::LockPoisoned(format!("{e}")))?;
            let status_str = state
                .client
                .put_host_command_acknowledgement(host_command_identifier, Some(&envelope))?;

            Ok(AckStatus::from_response(status_str.as_deref()))
        })
    }

    /// Get deployment specification.
    ///
    /// # Errors
    /// Returns error if the HTTP request fails or the response is missing required fields.
    pub fn get_deployment_specification(
        &self,
        deployment_execution_id: &str,
        host_identifier: &str,
    ) -> Result<DeploymentSpecification, CodeDeployClientError> {
        self.retry_on_throttle(|| {
            self.refresh_if_needed()?;
            let state = self
                .inner
                .read()
                .map_err(|e| CodeDeployClientError::LockPoisoned(format!("{e}")))?;
            let output = state
                .client
                .get_deployment_specification(deployment_execution_id, host_identifier)?;

            Ok(to_deployment_spec(output))
        })
    }

    /// Report command completion.
    ///
    /// # Errors
    /// Returns error if the HTTP request fails.
    pub fn put_host_command_complete(
        &self,
        host_command_identifier: &str,
        status: &str,
        diagnostics_payload: &str,
    ) -> Result<(), CodeDeployClientError> {
        self.retry_on_throttle(|| {
            self.refresh_if_needed()?;
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

            let state = self
                .inner
                .read()
                .map_err(|e| CodeDeployClientError::LockPoisoned(format!("{e}")))?;
            state.client.put_host_command_complete(
                host_command_identifier,
                command_status,
                Some(&envelope),
            )?;

            Ok(())
        })
    }

    /// The configured region.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.credentials.region
    }

    /// The agent credentials.
    #[must_use]
    pub fn credentials(&self) -> &Credentials {
        &self.credentials
    }

    /// The agent version sent on the `x-amz-codedeploy-agent-version` header.
    /// Returns `None` if the inner client lock is poisoned.
    #[must_use]
    pub fn agent_version(&self) -> Option<String> {
        self.inner.read().ok()?.client.agent_version().map(str::to_string)
    }
}

/// Convert a `SystemTime` credential expiry to an `Instant`.
///
/// Returns `None` if the expiry is in the past or the system clock is unreliable.
fn system_time_to_instant(expiry: std::time::SystemTime) -> Option<std::time::Instant> {
    let duration_until = expiry.duration_since(std::time::SystemTime::now()).ok()?;
    Some(std::time::Instant::now() + duration_until)
}

/// Convert agent credentials to AWS SDK credentials.
fn to_aws_credentials(creds: &Credentials) -> Result<AwsCredentials, CodeDeployClientError> {
    match &creds.mode {
        CredentialMode::IamUser { access_key_id, secret_access_key } => {
            Ok(AwsCredentials::new(access_key_id, secret_access_key, None, None, "codedeploy"))
        },
        CredentialMode::InstanceProfile => {
            // Fetch credentials from IMDS and record the real expiry on
            // `credential_expiry`; `refresh_if_needed()` re-fetches before expiry.
            // Returns Err if IMDS is unreachable — the agent cannot start without
            // valid credentials.
            use crate::aws_clients::imds;
            match imds::fetch_credentials() {
                Ok(aws_creds) => {
                    tracing::info!("Loaded credentials from IMDS instance profile");
                    Ok(aws_creds)
                },
                Err(e) => Err(CodeDeployClientError::ImdsUnavailable(e.to_string())),
            }
        },
        CredentialMode::IamSession { credentials_file } => {
            use crate::aws_clients::file_credentials;
            file_credentials::load_credentials_from_file(credentials_file)
                .map_err(|e| CodeDeployClientError::FileCredentialsError(e.to_string()))
        },
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

    fn write_session_credentials_file() -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[default]\naws_access_key_id = AKIASESSION\naws_secret_access_key = session_secret\naws_session_token = session_token_value"
        )
        .unwrap();
        file
    }

    fn iam_session_credentials(file: &tempfile::NamedTempFile) -> Credentials {
        Credentials {
            region: "us-east-1".to_string(),
            host_identifier: "arn:aws:sts::123:assumed-role/test".to_string(),
            mode: CredentialMode::IamSession { credentials_file: file.path().to_path_buf() },
        }
    }

    fn iam_session_credentials_bad_path() -> Credentials {
        Credentials {
            region: "us-east-1".to_string(),
            host_identifier: "arn:aws:sts::123:assumed-role/test".to_string(),
            mode: CredentialMode::IamSession {
                credentials_file: "/nonexistent/path/to/creds".into(),
            },
        }
    }

    #[test]
    fn new_client_with_instance_profile() {
        // On EC2/dev hosts: succeeds with real IMDS credentials.
        // Off EC2: fails with ImdsUnavailable (fail-fast, no IMDS).
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
    fn new_client_sets_agent_version_header() {
        // The client must report its version via the
        // x-amz-codedeploy-agent-version header (fleet version tracking).
        let client = CodeDeployCommandClient::new(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        assert_eq!(client.agent_version().as_deref(), Some(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn new_client_with_iam_session_succeeds() {
        let file = write_session_credentials_file();
        let client = CodeDeployCommandClient::new(
            iam_session_credentials(&file),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        assert_eq!(client.region(), "us-east-1");
        assert_eq!(client.credentials().host_identifier, "arn:aws:sts::123:assumed-role/test");
    }

    #[test]
    fn new_client_with_iam_session_bad_path_fails() {
        let result = CodeDeployCommandClient::new(
            iam_session_credentials_bad_path(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("file credentials"));
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
    fn refresh_if_needed_noop_for_iam_user() {
        let client = CodeDeployCommandClient::new(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        // Should be a no-op (no credential_expiry set)
        assert!(client.refresh_if_needed().is_ok());
    }

    #[test]
    fn refresh_if_needed_refreshes_near_expiry() {
        let file = write_session_credentials_file();
        let client = CodeDeployCommandClient::new(
            iam_session_credentials(&file),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        // Force expiry to be in the past
        client.inner.write().unwrap().credential_expiry =
            Some(std::time::Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
        assert!(client.refresh_if_needed().is_ok());
        // Expiry should be reset to ~30 min from now
        assert!(
            client.inner.read().unwrap().credential_expiry.unwrap() > std::time::Instant::now()
        );
    }

    #[test]
    fn refresh_if_needed_skips_when_not_near_expiry() {
        let file = write_session_credentials_file();
        let client = CodeDeployCommandClient::new(
            iam_session_credentials(&file),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        let original_expiry = client.inner.read().unwrap().credential_expiry.unwrap();
        // Not near expiry — should be a no-op
        assert!(client.refresh_if_needed().is_ok());
        assert_eq!(client.inner.read().unwrap().credential_expiry.unwrap(), original_expiry);
    }

    #[test]
    fn to_aws_credentials_instance_profile_returns_result() {
        // On EC2/dev hosts with IMDS: returns real credentials from instance profile.
        // Off EC2 (no IMDS): returns ImdsUnavailable error (fail-fast).
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
    fn to_aws_credentials_iam_session_valid() {
        let file = write_session_credentials_file();
        let creds = iam_session_credentials(&file);
        let aws_creds = to_aws_credentials(&creds).unwrap();
        assert_eq!(aws_creds.access_key_id(), "AKIASESSION");
        assert_eq!(aws_creds.secret_access_key(), "session_secret");
        assert_eq!(aws_creds.session_token(), Some("session_token_value"));
    }

    #[test]
    fn to_aws_credentials_iam_session_bad_path() {
        let creds = iam_session_credentials_bad_path();
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

        let e5 = CodeDeployClientError::FileCredentialsError("file not found".to_string());
        assert!(e5.to_string().contains("file credentials"));
        assert!(e5.to_string().contains("file not found"));

        let e6 = CodeDeployClientError::LockPoisoned("lock was poisoned".to_string());
        assert!(e6.to_string().contains("lock"));
        assert!(e6.to_string().contains("lock was poisoned"));
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

    #[test]
    fn credential_expiry_set_for_iam_session() {
        let file = write_session_credentials_file();
        let client = CodeDeployCommandClient::new(
            iam_session_credentials(&file),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        // IamSession uses fixed 30-min expiry
        let expiry = client.inner.read().unwrap().credential_expiry;
        assert!(expiry.is_some(), "IamSession should have credential_expiry set");
    }

    #[test]
    fn credential_expiry_none_for_iam_user() {
        let client = CodeDeployCommandClient::new(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        let expiry = client.inner.read().unwrap().credential_expiry;
        assert!(expiry.is_none(), "IamUser should not have credential_expiry");
    }

    #[test]
    fn system_time_to_instant_future_expiry() {
        let future = std::time::SystemTime::now() + Duration::from_hours(1);
        let result = system_time_to_instant(future);
        assert!(result.is_some(), "future expiry should convert to Some(Instant)");
        assert!(result.unwrap() > std::time::Instant::now());
    }

    #[test]
    fn system_time_to_instant_past_expiry() {
        let past = std::time::SystemTime::now() - Duration::from_mins(1);
        let result = system_time_to_instant(past);
        assert!(result.is_none(), "past expiry should return None");
    }

    #[test]
    fn refresh_if_needed_returns_error_when_file_deleted() {
        let file = write_session_credentials_file();
        let path = file.path().to_path_buf();
        let client = CodeDeployCommandClient::new(
            iam_session_credentials(&file),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        // Force expiry to be in the past
        client.inner.write().unwrap().credential_expiry =
            Some(std::time::Instant::now().checked_sub(Duration::from_secs(1)).unwrap());
        // Delete the credentials file
        std::fs::remove_file(&path).unwrap();
        // Refresh should fail because the file is gone
        let result = client.refresh_if_needed();
        assert!(result.is_err(), "refresh should fail when credentials file is deleted");
    }

    #[test]
    fn credential_expiry_for_mode_instance_profile_defaults_to_cooldown() {
        let before = std::time::Instant::now();
        let expiry = credential_expiry_for_mode(&CredentialMode::InstanceProfile, None);
        assert!(expiry.is_some(), "InstanceProfile with None imds_expiry should default to Some");
        let value = expiry.unwrap();
        // Should be approximately now + CREDENTIAL_REFRESH_BUFFER + 60s (cooldown floor)
        let expected_floor = before + CREDENTIAL_REFRESH_BUFFER + Duration::from_mins(1);
        assert!(value >= expected_floor, "expiry should be >= cooldown floor");
        assert!(
            value <= expected_floor + Duration::from_secs(1),
            "expiry should be approximately at cooldown floor"
        );
    }

    #[test]
    fn refresh_if_needed_tolerates_transient_failure_when_creds_valid() {
        let file = write_session_credentials_file();
        let path = file.path().to_path_buf();
        let client = CodeDeployCommandClient::new(
            iam_session_credentials(&file),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        // Set expiry to within the buffer window but NOT past actual expiry.
        // CREDENTIAL_REFRESH_BUFFER is 5 min, so set expiry to 2 min from now.
        client.inner.write().unwrap().credential_expiry =
            Some(std::time::Instant::now() + Duration::from_mins(2));
        // Delete the file so refresh attempt will fail
        std::fs::remove_file(&path).unwrap();
        // Should succeed because credentials haven't actually expired yet
        assert!(
            client.refresh_if_needed().is_ok(),
            "should tolerate transient failure when creds still valid"
        );
    }

    // === Throttle retry + circuit breaker tests ===

    #[test]
    fn is_throttle_true_for_429_error() {
        let inner_err = codedeploy_commands::Error::new(
            codedeploy_commands::ErrorKind::Throttled,
            "Rate exceeded",
        );
        let err = CodeDeployClientError::Service(inner_err);
        assert!(err.is_throttle());
    }

    #[test]
    fn is_throttle_false_for_non_429_error() {
        let inner_err = codedeploy_commands::Error::new(
            codedeploy_commands::ErrorKind::ServerException,
            "Internal Server Error",
        );
        let err = CodeDeployClientError::Service(inner_err);
        assert!(!err.is_throttle());
    }

    #[test]
    fn is_throttle_false_for_non_service_error() {
        let err = CodeDeployClientError::LockPoisoned("test".into());
        assert!(!err.is_throttle());
    }

    #[test]
    fn retry_on_throttle_succeeds_immediately_on_non_throttle() {
        let client = CodeDeployCommandClient::new(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        let mut calls = 0u32;
        let result = client.retry_on_throttle(|| {
            calls += 1;
            Ok::<_, CodeDeployClientError>(42)
        });
        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls, 1);
    }

    #[test]
    fn retry_on_throttle_retries_on_throttle_then_succeeds() {
        let client = CodeDeployCommandClient::new(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        // Override gate backoff to 1ms for fast testing
        client.throttle_gate.set_backoff_for_test(Duration::from_millis(1));
        let mut calls = 0u32;
        let result = client.retry_on_throttle(|| {
            calls += 1;
            if calls <= 2 {
                let err = codedeploy_commands::Error::new(
                    codedeploy_commands::ErrorKind::Throttled,
                    "Rate exceeded",
                );
                Err(CodeDeployClientError::Service(err))
            } else {
                Ok(99)
            }
        });
        assert_eq!(result.unwrap(), 99);
        assert_eq!(calls, 3); // 2 throttles + 1 success
        // Gate should be reset after success
        assert!(!client.throttle_gate.is_throttled());
    }

    #[test]
    fn retry_on_throttle_gives_up_after_max_retries() {
        let client = CodeDeployCommandClient::new(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        // Override gate backoff to 1ms for fast testing
        client.throttle_gate.set_backoff_for_test(Duration::from_millis(1));
        let mut calls = 0u32;
        let result: Result<(), _> = client.retry_on_throttle(|| {
            calls += 1;
            let err = codedeploy_commands::Error::new(
                codedeploy_commands::ErrorKind::Throttled,
                "Rate exceeded",
            );
            Err(CodeDeployClientError::Service(err))
        });
        assert!(result.is_err());
        assert!(result.unwrap_err().is_throttle());
        // 1 initial + 5 retries = 6 calls
        assert_eq!(calls, 6);
    }

    #[test]
    fn retry_on_throttle_does_not_retry_non_throttle_errors() {
        let client = CodeDeployCommandClient::new(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
        )
        .unwrap();
        let mut calls = 0u32;
        let result: Result<(), _> = client.retry_on_throttle(|| {
            calls += 1;
            Err(CodeDeployClientError::LockPoisoned("test".into()))
        });
        assert!(result.is_err());
        assert!(!result.unwrap_err().is_throttle());
        assert_eq!(calls, 1); // No retries for non-throttle errors
    }

    #[test]
    fn shared_throttle_gate_propagates_across_clients() {
        let gate = Arc::new(crate::aws_clients::ThrottleGate::new());
        let client1 = CodeDeployCommandClient::with_throttle_gate(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
            Arc::clone(&gate),
        )
        .unwrap();
        let client2 = CodeDeployCommandClient::with_throttle_gate(
            iam_user_credentials(),
            false,
            false,
            None,
            Duration::from_secs(80),
            None,
            Arc::clone(&gate),
        )
        .unwrap();

        // Trip via client1 → client2 sees it
        client1.throttle_gate.trip();
        assert!(client2.throttle_gate.is_throttled());

        // Reset via client2 → client1 sees it
        client2.throttle_gate.reset();
        assert!(!client1.throttle_gate.is_throttled());
    }
}
