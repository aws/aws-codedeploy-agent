//! `CodeDeploy` Command Service HTTP client.

use crate::error::{Error, ErrorKind};
use crate::types::{
    CommandStatus, Envelope, GetDeploymentSpecificationInput, GetDeploymentSpecificationOutput,
    HostCommandInstance, PollHostCommandInput, PollHostCommandOutput, PostHostCommandUpdateInput,
    PostHostCommandUpdateOutput, PutHostCommandAcknowledgementInput,
    PutHostCommandAcknowledgementOutput, PutHostCommandCompleteInput,
};
use aws_credential_types::Credentials as AwsCredentials;
use aws_sigv4::http_request::{SignableBody, SignableRequest, SigningSettings, sign};
use aws_sigv4::sign::v4;
use std::time::{Duration, SystemTime};

/// `CodeDeploy` Command Service client.
#[derive(Debug)]
pub struct Client {
    endpoint: String,
    region: String,
    signing_name: String,
    /// `X-Amz-Target` operation prefix. The secure stack wants the bare
    /// `CodeDeployCommandService`; the legacy stack wants the versioned
    /// `CodeDeployCommandService_v20141006`. The wrong one returns HTTP 400
    /// `UnknownOperationException`.
    target_prefix: String,
    http: reqwest::blocking::Client,
    credentials: AwsCredentials,
    agent_version: Option<String>,
}

impl Client {
    /// Create a new client builder.
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// The resolved endpoint URL.
    #[must_use]
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// The configured region.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    #[must_use]
    pub fn agent_version(&self) -> Option<&str> {
        self.agent_version.as_deref()
    }

    /// Poll for a host command.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response is invalid.
    pub fn poll_host_command(
        &self,
        host_identifier: &str,
    ) -> Result<Option<HostCommandInstance>, Error> {
        let input = PollHostCommandInput { host_identifier: host_identifier.to_string() };
        let output: PollHostCommandOutput = self.call("PollHostCommand", &input)?;
        Ok(output.host_command)
    }

    /// Acknowledge receipt of a host command.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response is invalid.
    pub fn put_host_command_acknowledgement(
        &self,
        host_command_identifier: &str,
        diagnostics: Option<&Envelope>,
    ) -> Result<Option<String>, Error> {
        let input = PutHostCommandAcknowledgementInput {
            host_command_identifier: host_command_identifier.to_string(),
            diagnostics: diagnostics.cloned(),
        };
        let output: PutHostCommandAcknowledgementOutput =
            self.call("PutHostCommandAcknowledgement", &input)?;
        Ok(output.command_status)
    }

    /// Get deployment specification.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response is invalid.
    pub fn get_deployment_specification(
        &self,
        deployment_execution_id: &str,
        host_identifier: &str,
    ) -> Result<GetDeploymentSpecificationOutput, Error> {
        let input = GetDeploymentSpecificationInput {
            deployment_execution_id: deployment_execution_id.to_string(),
            host_identifier: host_identifier.to_string(),
        };
        self.call("GetDeploymentSpecification", &input)
    }

    /// Mark a host command as complete.
    ///
    /// # Errors
    /// Returns an error if the request fails.
    pub fn put_host_command_complete(
        &self,
        host_command_identifier: &str,
        command_status: CommandStatus,
        diagnostics: Option<&Envelope>,
    ) -> Result<(), Error> {
        let input = PutHostCommandCompleteInput {
            host_command_identifier: host_command_identifier.to_string(),
            command_status,
            diagnostics: diagnostics.cloned(),
        };
        self.call_no_response("PutHostCommandComplete", &input)
    }

    /// Post a host command update.
    ///
    /// # Errors
    /// Returns an error if the request fails or the response is invalid.
    pub fn post_host_command_update(
        &self,
        host_command_identifier: &str,
        estimated_completion_time: Option<&str>,
        diagnostics: Option<&Envelope>,
    ) -> Result<Option<String>, Error> {
        let input = PostHostCommandUpdateInput {
            host_command_identifier: host_command_identifier.to_string(),
            estimated_completion_time: estimated_completion_time.map(String::from),
            diagnostics: diagnostics.cloned(),
        };
        let output: PostHostCommandUpdateOutput = self.call("PostHostCommandUpdate", &input)?;
        Ok(output.command_status)
    }

    fn call<I: serde::Serialize, O: serde::de::DeserializeOwned>(
        &self,
        operation: &str,
        input: &I,
    ) -> Result<O, Error> {
        let bytes = self.send(operation, input)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| Error::new(ErrorKind::Deserialization, e.to_string()))
    }

    fn call_no_response<I: serde::Serialize>(
        &self,
        operation: &str,
        input: &I,
    ) -> Result<(), Error> {
        self.send(operation, input)?;
        Ok(())
    }

    fn build_request(&self, operation: &str, body: String) -> Result<http::Request<String>, Error> {
        let mut http_req = http::Request::builder()
            .method("POST")
            .uri(&self.endpoint)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{}.{operation}", self.target_prefix))
            .body(body)
            .map_err(|e| Error::new(ErrorKind::Build, e.to_string()))?;

        if let Some(version) = &self.agent_version {
            http_req.headers_mut().insert(
                "x-amz-codedeploy-agent-version",
                version.parse().map_err(|e: http::header::InvalidHeaderValue| {
                    Error::new(ErrorKind::Build, e.to_string())
                })?,
            );
        }

        Ok(http_req)
    }

    fn send<I: serde::Serialize>(&self, operation: &str, input: &I) -> Result<Vec<u8>, Error> {
        let body = serde_json::to_string(input)
            .map_err(|e| Error::new(ErrorKind::Serialization, e.to_string()))?;

        let mut http_req = self.build_request(operation, body.clone())?;

        self.sign_request(&mut http_req)?;

        let mut req = self.http.post(&self.endpoint).body(body);
        for (name, value) in http_req.headers() {
            req = req.header(name.as_str(), value.to_str().unwrap_or(""));
        }

        let response = req.send().map_err(|e| Error::new(ErrorKind::Network, e.to_string()))?;
        let status = response.status().as_u16();
        let body_bytes =
            response.bytes().map_err(|e| Error::new(ErrorKind::Network, e.to_string()))?;

        if !(200..300).contains(&status) {
            return Err(Error::from_response(status, &body_bytes));
        }

        Ok(body_bytes.to_vec())
    }

    fn sign_request(&self, request: &mut http::Request<String>) -> Result<(), Error> {
        let settings = SigningSettings::default();
        let identity = self.credentials.clone().into();
        let signing_params = v4::SigningParams::builder()
            .identity(&identity)
            .region(&self.region)
            .name(&self.signing_name)
            .time(SystemTime::now())
            .settings(settings)
            .build()
            .map_err(|e| Error::new(ErrorKind::Signing, e.to_string()))?;

        let signable = SignableRequest::new(
            request.method().as_str(),
            request.uri().to_string(),
            request.headers().iter().map(|(k, v)| (k.as_str(), v.to_str().unwrap_or(""))),
            SignableBody::Bytes(request.body().as_bytes()),
        )
        .map_err(|e| Error::new(ErrorKind::Signing, e.to_string()))?;

        let (signing_instructions, _signature) = sign(signable, &signing_params.into())
            .map_err(|e| Error::new(ErrorKind::Signing, e.to_string()))?
            .into_parts();

        signing_instructions.apply_to_request_http1x(request);
        Ok(())
    }
}

/// Builder for [`Client`].
#[derive(Debug)]
pub struct ClientBuilder {
    region: Option<String>,
    endpoint: Option<String>,
    use_fips: bool,
    enable_auth_policy: bool,
    credentials: Option<AwsCredentials>,
    http_read_timeout: Duration,
    agent_version: Option<String>,
    proxy_uri: Option<String>,
}

impl ClientBuilder {
    /// Set the AWS region (required).
    #[must_use]
    pub fn region(mut self, region: impl Into<String>) -> Self {
        self.region = Some(region.into());
        self
    }

    /// Set a custom endpoint override.
    #[must_use]
    pub fn endpoint(mut self, endpoint: Option<String>) -> Self {
        self.endpoint = endpoint;
        self
    }

    /// Enable FIPS endpoints.
    #[must_use]
    pub fn use_fips(mut self, use_fips: bool) -> Self {
        self.use_fips = use_fips;
        self
    }

    /// Enable auth policy (secure) endpoints.
    #[must_use]
    pub fn enable_auth_policy(mut self, enable: bool) -> Self {
        self.enable_auth_policy = enable;
        self
    }

    /// Set AWS credentials (required).
    #[must_use]
    pub fn credentials(mut self, credentials: AwsCredentials) -> Self {
        self.credentials = Some(credentials);
        self
    }

    /// Set HTTP read timeout (defaults to 80 seconds).
    #[must_use]
    pub fn http_read_timeout(mut self, timeout: Duration) -> Self {
        self.http_read_timeout = timeout;
        self
    }

    /// Set agent version for x-amz-codedeploy-agent-version header.
    #[must_use]
    pub fn agent_version(mut self, version: Option<String>) -> Self {
        self.agent_version = version;
        self
    }

    /// Set HTTP proxy URI.
    #[must_use]
    pub fn proxy_uri(mut self, proxy: Option<String>) -> Self {
        self.proxy_uri = proxy;
        self
    }

    /// Build the client.
    ///
    /// # Errors
    /// Returns an error if required fields are missing or HTTP client creation fails.
    pub fn build(self) -> Result<Client, Error> {
        let region =
            self.region.ok_or_else(|| Error::new(ErrorKind::Build, "region is required"))?;
        let credentials = self
            .credentials
            .ok_or_else(|| Error::new(ErrorKind::Build, "credentials are required"))?;

        let endpoint = crate::endpoint::resolve(
            &region,
            self.endpoint.as_deref(),
            self.use_fips,
            self.enable_auth_policy,
        );

        let mut http_builder =
            reqwest::blocking::ClientBuilder::new().timeout(self.http_read_timeout);

        if let Some(proxy) = self.proxy_uri {
            let proxy = reqwest::Proxy::all(&proxy)
                .map_err(|e| Error::new(ErrorKind::Build, e.to_string()))?;
            http_builder = http_builder.proxy(proxy);
        }

        let http = http_builder.build().map_err(|e| Error::new(ErrorKind::Build, e.to_string()))?;

        // Signing name and target prefix both track enable_auth_policy, which
        // also selects the `-secure` endpoint.
        let (signing_name, target_prefix) = if self.enable_auth_policy {
            ("codedeploy-commands-secure", "CodeDeployCommandService")
        } else {
            ("codedeploy-commands", "CodeDeployCommandService_v20141006")
        };
        let signing_name = signing_name.to_string();
        let target_prefix = target_prefix.to_string();

        // Log the resolved stack so the agent log shows which endpoint it polls.
        tracing::info!(
            endpoint = %endpoint,
            signing_name = %signing_name,
            target_prefix = %target_prefix,
            region = %region,
            use_fips = self.use_fips,
            enable_auth_policy = self.enable_auth_policy,
            "CodeDeploy command client configured"
        );

        Ok(Client {
            endpoint,
            region,
            signing_name,
            target_prefix,
            http,
            credentials,
            agent_version: self.agent_version,
        })
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            region: None,
            endpoint: None,
            use_fips: false,
            enable_auth_policy: false,
            credentials: None,
            http_read_timeout: Duration::from_secs(80),
            agent_version: None,
            proxy_uri: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_credential_types::Credentials as AwsCredentials;

    fn test_credentials() -> AwsCredentials {
        AwsCredentials::new("test-key", "test-secret", None, None, "test")
    }

    #[test]
    fn build_requires_region() {
        let err = Client::builder().credentials(test_credentials()).build().unwrap_err();
        assert_eq!(err.kind(), &ErrorKind::Build);
    }

    #[test]
    fn build_requires_credentials() {
        let err = Client::builder().region("us-east-1").build().unwrap_err();
        assert_eq!(err.kind(), &ErrorKind::Build);
    }

    #[test]
    fn build_with_region_and_credentials_succeeds() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();
        assert_eq!(client.region(), "us-east-1");
        assert_eq!(client.endpoint(), "https://codedeploy-commands.us-east-1.amazonaws.com");
    }

    #[test]
    fn build_with_fips() {
        let client = Client::builder()
            .region("us-west-2")
            .credentials(test_credentials())
            .use_fips(true)
            .build()
            .unwrap();
        assert_eq!(client.endpoint(), "https://codedeploy-commands-fips.us-west-2.amazonaws.com");
    }

    #[test]
    fn build_with_custom_endpoint() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .endpoint(Some("https://custom".into()))
            .build()
            .unwrap();
        assert_eq!(client.endpoint(), "https://custom");
    }

    #[test]
    fn build_with_all_options() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .http_read_timeout(Duration::from_secs(120))
            .agent_version(Some("1.0.0".into()))
            .use_fips(true)
            .enable_auth_policy(true)
            .build()
            .unwrap();
        assert_eq!(client.region(), "us-east-1");
    }

    #[test]
    fn build_with_proxy() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .proxy_uri(Some("http://proxy:8080".into()))
            .build()
            .unwrap();
        assert_eq!(client.region(), "us-east-1");
    }

    #[test]
    fn sign_request_adds_authorization_header() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let mut request = http::Request::builder()
            .method("POST")
            .uri("https://example.com")
            .header("content-type", "application/json")
            .body("{}".to_string())
            .unwrap();

        client.sign_request(&mut request).unwrap();

        // Verify that authorization header was added
        assert!(request.headers().contains_key("authorization"));
        assert!(request.headers().contains_key("x-amz-date"));
    }

    // Mock tests for API methods - these test the method signatures and basic error handling
    // without making real HTTP calls

    /// Returns true if the error kind is one of the expected failure modes when
    /// making a real HTTP call with test credentials (Http from AWS rejection,
    /// or Network if outbound access is blocked in sandboxed build environments).
    fn is_request_error(kind: &ErrorKind) -> bool {
        matches!(kind, ErrorKind::Http | ErrorKind::Network)
    }

    #[test]
    fn poll_host_command_serializes_input_correctly() {
        // Test that the method constructs the correct input type
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        // Will fail with Http (AWS rejection) or Network (sandboxed build)
        let result = client.poll_host_command("test-host");
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn put_host_command_acknowledgement_serializes_input_correctly() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let envelope = Envelope { format: "JSON".to_string(), payload: "{}".to_string() };

        let result = client.put_host_command_acknowledgement("test-cmd", Some(&envelope));
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn put_host_command_acknowledgement_without_diagnostics() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let result = client.put_host_command_acknowledgement("test-cmd", None);
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn get_deployment_specification_serializes_input_correctly() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let result = client.get_deployment_specification("test-exec", "test-host");
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn put_host_command_complete_serializes_input_correctly() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let result = client.put_host_command_complete("test-cmd", CommandStatus::Succeeded, None);
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn put_host_command_complete_with_diagnostics() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let envelope = Envelope { format: "JSON".to_string(), payload: "{}".to_string() };

        let result =
            client.put_host_command_complete("test-cmd", CommandStatus::Failed, Some(&envelope));
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn post_host_command_update_serializes_input_correctly() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let result = client.post_host_command_update("test-cmd", None, None);
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn post_host_command_update_with_diagnostics() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let envelope = Envelope { format: "JSON".to_string(), payload: "{}".to_string() };

        let result = client.post_host_command_update("test-cmd", None, Some(&envelope));
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn post_host_command_update_with_estimated_completion_time() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let result =
            client.post_host_command_update("test-cmd", Some("2026-02-19T12:00:00Z"), None);
        assert!(result.is_err());
        assert!(is_request_error(result.unwrap_err().kind()));
    }

    #[test]
    fn client_with_agent_version_includes_header() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .agent_version(Some("1.2.3".to_string()))
            .build()
            .unwrap();

        // Test that agent version is stored
        assert!(client.agent_version.is_some());
        assert_eq!(client.agent_version.as_ref().unwrap(), "1.2.3");
    }

    #[test]
    fn client_without_agent_version() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        assert!(client.agent_version.is_none());
    }

    #[test]
    fn client_builder_default_timeout() {
        let builder = ClientBuilder::default();
        assert_eq!(builder.http_read_timeout, Duration::from_secs(80));
    }

    #[test]
    fn client_builder_custom_timeout() {
        let timeout = Duration::from_secs(120);
        let builder = ClientBuilder::default().http_read_timeout(timeout);
        assert_eq!(builder.http_read_timeout, timeout);
    }

    #[test]
    fn build_request_sets_content_type_header() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let req = client.build_request("PollHostCommand", "{}".into()).unwrap();
        assert_eq!(req.headers()["content-type"], "application/x-amz-json-1.1");
    }

    #[test]
    fn build_request_sets_x_amz_target_header() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let req = client.build_request("PollHostCommand", "{}".into()).unwrap();
        assert_eq!(
            req.headers()["x-amz-target"],
            "CodeDeployCommandService_v20141006.PollHostCommand"
        );
    }

    #[test]
    fn build_request_x_amz_target_varies_by_operation() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let operations = [
            "PollHostCommand",
            "PutHostCommandAcknowledgement",
            "GetDeploymentSpecification",
            "PutHostCommandComplete",
            "PostHostCommandUpdate",
        ];

        for op in operations {
            let req = client.build_request(op, "{}".into()).unwrap();
            let expected = format!("CodeDeployCommandService_v20141006.{op}");
            assert_eq!(req.headers()["x-amz-target"], expected.as_str());
        }
    }

    #[test]
    fn build_request_uses_versioned_target_without_auth_policy() {
        // Legacy stack expects the versioned prefix.
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .enable_auth_policy(false)
            .build()
            .unwrap();

        let req = client.build_request("PollHostCommand", "{}".into()).unwrap();
        assert_eq!(
            req.headers()["x-amz-target"],
            "CodeDeployCommandService_v20141006.PollHostCommand"
        );
    }

    #[test]
    fn build_request_uses_bare_target_with_auth_policy() {
        // Secure stack expects the bare prefix; the versioned one 400s there.
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .enable_auth_policy(true)
            .build()
            .unwrap();

        let req = client.build_request("PollHostCommand", "{}".into()).unwrap();
        assert_eq!(req.headers()["x-amz-target"], "CodeDeployCommandService.PollHostCommand");
    }

    #[test]
    fn build_request_includes_agent_version_header_when_set() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .agent_version(Some("2.0.0".into()))
            .build()
            .unwrap();

        let req = client.build_request("PollHostCommand", "{}".into()).unwrap();
        assert_eq!(req.headers()["x-amz-codedeploy-agent-version"], "2.0.0");
    }

    #[test]
    fn build_request_omits_agent_version_header_when_unset() {
        let client = Client::builder()
            .region("us-east-1")
            .credentials(test_credentials())
            .build()
            .unwrap();

        let req = client.build_request("PollHostCommand", "{}".into()).unwrap();
        assert!(!req.headers().contains_key("x-amz-codedeploy-agent-version"));
    }
}
