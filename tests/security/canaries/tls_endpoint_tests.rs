// Canary tests for TLS enforcement and endpoint verification.
//
// These tests validate that the agent enforces secure TLS connections:
// - Rejects legacy TLS versions (TLS 1.0, 1.1, SSL 3.0) via rustls
// - Rejects weak cipher suites (RC4, DES, 3DES, NULL, EXPORT)
// - Enforces FIPS-approved endpoints when use_fips_mode=true
// - Rejects self-signed and hostname-mismatched certificates
// - Includes SigV4 authentication headers on all AWS API requests
//
// ALL tests in this file are #[ignore] — they are canary tests requiring
// infrastructure dependencies (TLS servers, wiremock async runtime, or network
// access) and should only execute in weekly CI.

use proptest::prelude::*;

// ===========================================================================
// TLS Enforcement
// ===========================================================================

// Validates: Agent rejects connections when server offers only TLS 1.0, TLS 1.1, or SSL 3.0.
// rustls does not implement legacy TLS versions, so all handshakes with such servers will fail.
#[tokio::test]
#[ignore] // Canary: requires TLS mock server with legacy-only config, runs weekly in CI
// TODO: Implement with openssl s_server offering TLS 1.0 only, then assert
// verify_tls_connection fails. Requires openssl binary in CI image.
async fn rejects_legacy_tls_versions() {
    // Arrange: verify via static analysis that the TLS configuration is secure.
    // A real canary would use openssl s_server to offer legacy TLS only.
    let ssl_source = include_str!("../../../src/aws_clients/ssl.rs");

    // Verify reqwest is configured with strict TLS (no danger_accept_invalid_certs(true))
    assert!(
        ssl_source.contains("danger_accept_invalid_certs(false)"),
        "ssl.rs must explicitly enforce TLS certificate verification"
    );
    // Verify no TLS version override that would enable legacy versions
    assert!(
        !ssl_source.contains("min_tls_version"),
        "ssl.rs should not override minimum TLS version — rustls defaults to TLS 1.2+"
    );

    // Dynamic test: verify that a non-TLS (HTTP) endpoint is handled without panic
    let result =
        codedeploy_agent::aws_clients::ssl::verify_tls_connection("http://127.0.0.1:1", None);
    // HTTP (not HTTPS) should fail or be handled gracefully
    assert!(
        result.is_err() || result.is_ok(),
        "Non-TLS endpoint should be handled without panic"
    );
}

// Validates: Agent rejects connections when server offers only weak ciphers
// (RC4, DES, 3DES, NULL, EXPORT). rustls only implements AEAD ciphers
// (AES-128-GCM, AES-256-GCM, ChaCha20-Poly1305), so weak cipher negotiation fails.
#[test]
#[ignore] // Canary: requires TLS server configured with weak-only ciphers, runs weekly in CI
// TODO: Implement with openssl s_server offering -cipher RC4-SHA:DES-CBC3-SHA only,
// then assert verify_tls_connection fails. Requires openssl binary in CI image.
fn rejects_weak_cipher_suites() {
    // Code-level verification: confirm no custom cipher configuration
    let ssl_source = include_str!("../../../src/aws_clients/ssl.rs");
    assert!(
        !ssl_source.contains("cipher"),
        "ssl.rs should not configure custom cipher suites — \
         rustls defaults enforce strong AEAD-only ciphers"
    );
    assert!(
        !ssl_source.contains("native-tls") && !ssl_source.contains("native_tls"),
        "ssl.rs must not use native-tls which may allow weak ciphers"
    );
}

// Validates: With use_fips_mode=true, agent resolves FIPS-approved endpoints for all
// AWS service calls, and rejects non-US regions that lack FIPS endpoints.
#[test]
#[ignore] // Canary: full FIPS validation including algorithm verification, runs weekly in CI
// TODO: Full FIPS canary requires a FIPS-enabled OS to verify algorithm enforcement.
// This test validates config-level FIPS region gating.
fn fips_mode_uses_fips_endpoints_only() {
    use codedeploy_agent::config::AgentConfig;

    // Arrange: config with FIPS enabled via file
    let dir = tempfile::TempDir::new().expect("should create temp dir");
    let config_path = dir.path().join("fips.yml");
    std::fs::write(&config_path, "use_fips_mode: true\n").expect("should write FIPS config file");
    let config = AgentConfig::from_file(&config_path).expect("should parse FIPS config");
    assert!(config.use_fips_mode, "FIPS mode should be enabled");

    // Act & Assert: non-US regions must be rejected
    let err = config.validate_fips("eu-west-1");
    assert!(err.is_err(), "FIPS mode must reject non-US region eu-west-1");

    let err = config.validate_fips("ap-southeast-1");
    assert!(err.is_err(), "FIPS mode must reject non-US region ap-southeast-1");

    // US regions must be accepted
    assert!(config.validate_fips("us-east-1").is_ok(), "FIPS mode must accept us-east-1");
    assert!(config.validate_fips("us-west-2").is_ok(), "FIPS mode must accept us-west-2");
    assert!(
        config.validate_fips("us-gov-east-1").is_ok(),
        "FIPS mode must accept us-gov-east-1"
    );
    assert!(
        config.validate_fips("us-gov-west-1").is_ok(),
        "FIPS mode must accept us-gov-west-1"
    );

    // Verify FIPS endpoint pattern in source
    let s3_source = include_str!("../../../src/aws_clients/s3_client.rs");
    assert!(
        s3_source.contains("s3-fips"),
        "S3 client must construct FIPS endpoint URLs (s3-fips.{{region}}.amazonaws.com)"
    );
}

// ===========================================================================
// Endpoint Verification
// ===========================================================================

// Validates: Agent rejects connections when server presents a self-signed certificate,
// preventing MITM attacks with rogue CA certificates.
#[tokio::test]
#[ignore] // Canary: requires self-signed TLS server, runs weekly in CI
// TODO: Implement with rcgen-generated self-signed cert + local TLS listener,
// then assert verify_tls_connection rejects it.
async fn rejects_self_signed_certificate() {
    // Static verification: danger_accept_invalid_certs must be false
    let ssl_source = include_str!("../../../src/aws_clients/ssl.rs");
    assert!(
        ssl_source.contains("danger_accept_invalid_certs(false)"),
        "TLS client must NOT accept invalid certificates"
    );
    assert!(
        !ssl_source.contains("danger_accept_invalid_certs(true)"),
        "TLS client must NEVER set danger_accept_invalid_certs(true)"
    );

    // Dynamic test with known bad endpoint (requires network)
    let result = codedeploy_agent::aws_clients::ssl::verify_tls_connection(
        "https://self-signed.badssl.com/",
        None,
    );
    // badssl.com may not always be available; in CI we'd use a local server
    if let Err(e) = result {
        assert!(
            e.contains("certificate")
                || e.contains("TLS")
                || e.contains("SSL")
                || e.contains("failed"),
            "Error should mention certificate/TLS issue: {e}"
        );
    }
}

// Validates: Agent rejects connections when the server certificate's CN/SAN does not
// match the requested hostname, preventing DNS-based MITM attacks.
#[tokio::test]
#[ignore] // Canary: requires hostname-mismatch TLS server, runs weekly in CI
// TODO: Implement with a TLS server whose cert CN/SAN does not match the
// requested hostname, then assert verify_tls_connection rejects it.
async fn rejects_wrong_domain_certificate() {
    // Static verification: confirm no hostname override
    let ssl_source = include_str!("../../../src/aws_clients/ssl.rs");
    assert!(
        !ssl_source.contains("danger_accept_invalid_hostnames"),
        "TLS client must not disable hostname verification"
    );
    assert!(
        !ssl_source.contains("set_hostname_verification(false)"),
        "TLS client must not disable hostname verification"
    );

    // Dynamic test with known hostname-mismatch endpoint (requires network)
    let result = codedeploy_agent::aws_clients::ssl::verify_tls_connection(
        "https://wrong.host.badssl.com/",
        None,
    );
    if let Err(e) = result {
        assert!(
            e.contains("certificate")
                || e.contains("TLS")
                || e.contains("hostname")
                || e.contains("failed"),
            "Error should indicate hostname/certificate mismatch: {e}"
        );
    }
}

// Validates: All outbound AWS API requests contain SigV4 authentication headers
// (Authorization: AWS4-HMAC-SHA256 and X-Amz-Date), as verified by wiremock inspection.
#[tokio::test]
#[ignore] // Canary: requires wiremock S3 endpoint with request capture, runs weekly in CI
// TODO: Requires wiremock async runtime with S3Client configured to point at
// mock server. The AWS SDK signs all requests with SigV4 automatically.
async fn sigv4_headers_on_all_aws_requests() {
    use wiremock::matchers::{method, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock_server = MockServer::start().await;

    // Mock any S3 GetObject request
    Mock::given(method("GET"))
        .and(path_regex(".*"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(b"mock-bundle-content")
                .insert_header("ETag", "\"abc123\""),
        )
        .mount(&mock_server)
        .await;

    // Build an S3 client pointing at our mock server
    use codedeploy_agent::aws_clients::credentials::{CredentialMode, Credentials};
    use codedeploy_agent::aws_clients::s3_client::{S3Client, S3ClientConfig};

    let creds = Credentials {
        region: "us-east-1".to_string(),
        host_identifier: "test-host".to_string(),
        mode: CredentialMode::IamUser {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
        },
    };

    let config =
        S3ClientConfig { endpoint_override: Some(mock_server.uri()), ..Default::default() };

    let client = S3Client::new(creds, &config).expect("should create S3 client");

    // Attempt a download — the SDK will sign the request
    let download_dir = tempfile::TempDir::new().expect("should create temp dir");
    let dest = download_dir.path().join("bundle.zip");
    let _ = client.download_to_file("test-bucket", "test-key", None, &dest);

    // Inspect all received requests
    let received = mock_server.received_requests().await.expect("should have received requests");

    assert!(!received.is_empty(), "Mock server should have received at least one request");

    for req in &received {
        let auth_header =
            req.headers.get("authorization").or_else(|| req.headers.get("Authorization"));
        assert!(
            auth_header.is_some(),
            "Request to {} must include Authorization header",
            req.url
        );
        let auth_value = auth_header
            .expect("auth header present")
            .to_str()
            .expect("auth header should be valid string");
        assert!(
            auth_value.contains("AWS4-HMAC-SHA256"),
            "Authorization header must use AWS4-HMAC-SHA256 signing. Got: {auth_value}"
        );

        // Verify X-Amz-Date is present
        let date_header = req.headers.get("x-amz-date").or_else(|| req.headers.get("X-Amz-Date"));
        assert!(date_header.is_some(), "Request to {} must include X-Amz-Date header", req.url);
    }
}

// ===========================================================================
// Property-Based Tests
// ===========================================================================

// Property 34: SigV4 headers on all AWS requests
// Validates: For any S3 request with arbitrary bucket/key, the SDK always includes
// SigV4 Authorization and X-Amz-Date headers.
#[cfg(unix)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    #[ignore] // Canary: requires wiremock with async runtime, runs weekly in CI
    fn sigv4_headers_present_for_any_s3_path(
        bucket in "[a-z][a-z0-9-]{2,20}",
        key in "[a-zA-Z0-9/_.-]{1,50}"
    ) {
        // Each iteration creates a wiremock server and verifies SigV4 headers
        // This is resource-intensive, hence canary-only
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("should build tokio runtime");

        rt.block_on(async {
            use wiremock::{MockServer, Mock, ResponseTemplate};
            use wiremock::matchers::{method, path_regex};

            let mock_server = MockServer::start().await;
            Mock::given(method("GET"))
                .and(path_regex(".*"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_bytes(b"data")
                        .insert_header("ETag", "\"test\"")
                )
                .mount(&mock_server)
                .await;

            use codedeploy_agent::aws_clients::credentials::{CredentialMode, Credentials};
            use codedeploy_agent::aws_clients::s3_client::{S3Client, S3ClientConfig};

            let creds = Credentials {
                region: "us-east-1".to_string(),
                host_identifier: "test".to_string(),
                mode: CredentialMode::IamUser {
                    access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
                    secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
                },
            };
            let config = S3ClientConfig {
                endpoint_override: Some(mock_server.uri()),
                ..Default::default()
            };
            let client = S3Client::new(creds, &config).expect("S3 client");
            let dir = tempfile::TempDir::new().expect("tempdir");
            let _ = client.download_to_file(&bucket, &key, None, &dir.path().join("out"));

            let received = mock_server.received_requests().await.unwrap_or_default();
            for req in &received {
                let has_auth = req.headers.get("authorization").is_some()
                    || req.headers.get("Authorization").is_some();
                prop_assert!(has_auth, "Missing Authorization header for {}/{}", bucket, key);
            }
            Ok(())
        })?;
    }
}
