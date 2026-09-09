//! S3 client for downloading objects.
//!
//! Wraps `aws-sdk-s3` with agent-specific configuration: endpoint override,
//! FIPS mode, proxy, and signature v4 (SDK default).
//!
//! Downloads use a single `GetObject` request with streaming body.
//! For very large bundles, consider switching to `aws-s3-transfer-manager`
//! which splits downloads into parallel range requests for higher throughput.
//!
//! ## Wire logging (`log_aws_wire`)
//!
//! When the `log_aws_wire` config setting is enabled, [`S3ClientConfig::wire_log`]
//! is populated and a [`super::wire_log::WireLogInterceptor`] is attached to this
//! client. It appends every S3 request/response to a dedicated, size-rotated,
//! restricted-permission `<program_name>.aws_wire.log` (64 MB × 16 chunks ≈ 1 GB).
//!
//! ## TLS trust store (cross-platform)
//!
//! The S3 client uses `aws-smithy-http-client` with `rustls-aws-lc`. The default
//! `TrustStore` loads the **platform system trust store** via `rustls-native-certs`
//! (`enable_native_roots: true`), which reads OS-managed CA certificates
//! (Windows cert store, `/etc/ssl/certs` on Linux, Keychain on macOS). Enterprise
//! or custom CAs added to the OS store are trusted automatically.
//!
//! When `AWS_SSL_CA_DIRECTORY` is set, all `*.pem` files from that directory are
//! loaded as **additional** trusted CA certificates on top of the native roots.
//!
//! The agent uses `AWS_SSL_CA_DIRECTORY` (a directory of PEM files) rather than
//! a single CA bundle file, which is strictly more flexible.

use super::aws_client::AwsClient;
use super::credentials::{CredentialMode, Credentials};
use std::io::{self, Write};
use std::path::Path;
use tracing::{debug, info};

/// Streaming buffer size for S3 downloads.
const STREAM_BUFFER_SIZE: usize = 8 * 1024 * 1024;

/// Load additional CA certificates from `AWS_SSL_CA_DIRECTORY` into a
/// [`TrustStore`], if the env var is set and points at a directory with `*.pem`
/// files. Returns `Some(trust_store)` only when at least one cert was loaded.
///
/// [`TrustStore`]: aws_smithy_http_client::tls::TrustStore
fn load_ca_trust_store(ca_dir: Option<String>) -> Option<aws_smithy_http_client::tls::TrustStore> {
    let ca_dir = ca_dir?;
    let path = Path::new(&ca_dir);
    if !path.is_dir() {
        debug!("AWS_SSL_CA_DIRECTORY={ca_dir} is not a directory, skipping custom CA certs");
        return None;
    }

    let mut trust_store = aws_smithy_http_client::tls::TrustStore::default();
    let entries = match std::fs::read_dir(path) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Failed to read AWS_SSL_CA_DIRECTORY={ca_dir}: {e}");
            return None;
        },
    };
    let mut loaded: usize = 0;
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!("Error reading entry in AWS_SSL_CA_DIRECTORY={ca_dir}: {e}");
                continue;
            },
        };
        let file_path = entry.path();
        if file_path.extension().and_then(|e| e.to_str()) == Some("pem") {
            match std::fs::read(&file_path) {
                Ok(pem_data) => {
                    trust_store.add_pem_certificate(pem_data);
                    loaded += 1;
                    debug!("Loaded CA cert from {}", file_path.display());
                },
                Err(e) => {
                    tracing::warn!("Failed to read PEM file {}: {e}", file_path.display());
                },
            }
        }
    }

    if loaded == 0 {
        debug!("No .pem files found in AWS_SSL_CA_DIRECTORY={ca_dir}");
        return None;
    }
    Some(trust_store)
}

/// Build a custom HTTPS client when a proxy and/or extra CA certs are configured.
///
/// Returns `Some` when either a `proxy_uri` is given or `AWS_SSL_CA_DIRECTORY`
/// yields custom CA certs; `None` otherwise (letting the SDK use its default HTTP
/// client, which still auto-detects the `HTTP_PROXY`/`HTTPS_PROXY` env vars).
///
/// Wiring the YAML `proxy_uri` through here (in addition to the proxy env vars)
/// lets a single `proxy_uri` setting route all agent egress (polling, GitHub,
/// and S3 bundle downloads).
///
/// Build the `NO_PROXY` rule list that keeps IMDS credential fetches off the
/// proxy. Always includes the default IMDS addresses (IPv4 `169.254.169.254` and
/// IPv6 `[fd00:ec2::254]`); when `imds_endpoint_override` is set (the SDK's
/// `AWS_EC2_METADATA_SERVICE_ENDPOINT`, a full URL like `http://host:port`), its
/// host is appended so a custom IMDS endpoint is bypassed too.
///
/// IMDS is link-local / instance-local, so a remote proxy can never reach it;
/// routing credential fetches through the proxy breaks instance-profile creds.
fn imds_no_proxy_rules(imds_endpoint_override: Option<String>) -> String {
    let mut rules = String::from("169.254.169.254,[fd00:ec2::254]");
    if let Some(ep) = imds_endpoint_override {
        // Strip the scheme (`http://host:port` -> `host:port`) and any trailing
        // slash. A NO_PROXY rule matches on host, so keeping an optional `:port`
        // is harmless; not splitting on `:` keeps IPv6 literals intact.
        let host = ep.split_once("://").map_or(ep.as_str(), |(_, rest)| rest).trim_end_matches('/');
        if !host.is_empty() {
            rules.push(',');
            rules.push_str(host);
        }
    }
    rules
}

/// # Parameters
///
/// * `ca_dir` — value of `AWS_SSL_CA_DIRECTORY` env var, passed explicitly for testability.
/// * `proxy_uri` — the agent's configured `proxy_uri`, applied to all schemes.
fn build_custom_http_client(
    ca_dir: Option<String>,
    proxy_uri: Option<&str>,
) -> Option<aws_sdk_s3::config::SharedHttpClient> {
    let trust_store = load_ca_trust_store(ca_dir);

    // Build a proxy config that routes every scheme through `proxy_uri`, matching
    // the command-control and GitHub download clients (both use "all schemes").
    //
    // CRITICAL: exclude the IMDS address from the proxy. The S3 client's
    // credential provider fetches instance-profile credentials from IMDS; IMDS is
    // link-local and only reachable from the instance itself, so a remote proxy
    // cannot connect to it on the agent's behalf. Without this bypass, `proxy_uri`
    // + instance-profile creds fails with "failed to load IMDS session token:
    // request forbidden" and S3 bundle downloads break (regression from the 1.8.x
    // agent, which excluded IMDS).
    let no_proxy = imds_no_proxy_rules(std::env::var("AWS_EC2_METADATA_SERVICE_ENDPOINT").ok());
    let proxy_config = match proxy_uri {
        Some(uri) => match aws_smithy_http_client::proxy::ProxyConfig::all(uri) {
            Ok(cfg) => Some(cfg.no_proxy(&no_proxy)),
            Err(e) => {
                tracing::warn!("Invalid proxy_uri '{uri}' for S3 client, ignoring: {e}");
                None
            },
        },
        None => None,
    };

    // Nothing custom to configure — let the SDK build its default client (which
    // still honors HTTP_PROXY/HTTPS_PROXY env vars on its own).
    if trust_store.is_none() && proxy_config.is_none() {
        return None;
    }

    // that grcov cannot attribute to source lines.
    // Build the TLS context once (custom CA certs, if any). `proxy_config` lives
    // on the low-level `ConnectorBuilder`, not the high-level `Builder`, so we
    // assemble the connector per-invocation inside `build_with_connector_fn`
    // (the supported hook for a fully custom connector). The connector honors the
    // SDK's per-request `HttpConnectorSettings` (timeouts, etc.) passed to it.
    let tls_context = trust_store.and_then(|ts| {
        match aws_smithy_http_client::tls::TlsContext::builder().with_trust_store(ts).build() {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                tracing::warn!("Failed to build TLS context from AWS_SSL_CA_DIRECTORY: {e}");
                None
            },
        }
    });

    Some(aws_smithy_http_client::Builder::new().build_with_connector_fn(
        move |settings, _runtime_components| {
            let mut cb = aws_smithy_http_client::Connector::builder().tls_provider(
                aws_smithy_http_client::tls::Provider::Rustls(
                    aws_smithy_http_client::tls::rustls_provider::CryptoMode::AwsLc,
                ),
            );
            if let Some(settings) = settings {
                cb = cb.connector_settings(settings.clone());
            }
            if let Some(ctx) = tls_context.clone() {
                cb = cb.tls_context(ctx);
            }
            if let Some(pc) = proxy_config.clone() {
                cb = cb.proxy_config(pc);
            }
            cb.build()
        },
    ))
}

/// Configuration for S3 client construction.
#[derive(Debug, Default)]
pub struct S3ClientConfig {
    /// Custom S3 endpoint URL (overrides default).
    pub endpoint_override: Option<String>,
    /// Use FIPS endpoints (`https://s3-fips.{region}.amazonaws.com`).
    pub use_fips: bool,
    /// HTTP proxy URI (from the agent's `proxy_uri` config). Applied to the S3
    /// client so bundle downloads route through the same proxy as the polling
    /// and GitHub clients. When `None`, the SDK still auto-detects the
    /// `HTTP_PROXY`/`HTTPS_PROXY` environment variables.
    pub proxy_uri: Option<String>,
    /// When set, attach an HTTP wire-trace interceptor writing S3 request/
    /// response traces to `<log_dir>/<program_name>.aws_wire.log` (restricted
    /// `0640`). Populated from the `log_aws_wire` config setting; `None` means
    /// wire logging is disabled (the default). The tuple is
    /// `(log_dir, program_name)`.
    pub wire_log: Option<(std::path::PathBuf, String)>,
}

impl S3ClientConfig {
    fn resolve_endpoint(&self, region: &str) -> Option<String> {
        if let Some(endpoint) = &self.endpoint_override {
            debug!("Using S3 override endpoint {endpoint}");
            return Some(endpoint.clone());
        }
        if self.use_fips {
            debug!("Using FIPS endpoint");
            return Some(format!("https://s3-fips.{region}.amazonaws.com"));
        }
        None
    }
}

#[derive(Debug)]
pub struct S3Client {
    credentials: Credentials,
    inner: aws_sdk_s3::Client,
    runtime: tokio::runtime::Runtime,
}

impl S3Client {
    /// Create a new S3 client.
    ///
    /// # Errors
    /// Returns an error if the tokio runtime cannot be created.
    pub fn new(credentials: Credentials, config: &S3ClientConfig) -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build()?;

        let inner = runtime.block_on(Self::build_client(&credentials, config))?;

        Ok(Self { credentials, inner, runtime })
    }

    async fn build_client(
        credentials: &Credentials,
        config: &S3ClientConfig,
    ) -> io::Result<aws_sdk_s3::Client> {
        let mut config_loader = aws_config::from_env()
            .region(aws_sdk_s3::config::Region::new(credentials.region.clone()));

        // Inject agent credentials instead of relying on the default chain.
        // The same IAM user / instance-profile credentials are used for S3
        // downloads as for the CodeDeploy command service.
        match &credentials.mode {
            CredentialMode::IamUser { access_key_id, secret_access_key } => {
                config_loader =
                    config_loader.credentials_provider(aws_credential_types::Credentials::new(
                        access_key_id,
                        secret_access_key,
                        None,
                        None,
                        "codedeploy-s3",
                    ));
            },
            CredentialMode::IamSession { credentials_file } => {
                // Fail fast on obvious misconfiguration (wrong path, missing file).
                if !credentials_file.exists() {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!(
                            "IamSession credentials file not found: {}",
                            credentials_file.display()
                        ),
                    ));
                }
                // Auto-refreshing provider: re-reads file on expiry.
                config_loader = config_loader.credentials_provider(
                    super::file_credentials::FileCredentialProvider::new(credentials_file.clone()),
                );
            },
            CredentialMode::InstanceProfile => {
                // Let the SDK default chain handle IMDS (includes auto-refresh).
            },
        }

        if let Some(http_client) = build_custom_http_client(
            std::env::var("AWS_SSL_CA_DIRECTORY").ok(),
            config.proxy_uri.as_deref(),
        ) {
            config_loader = config_loader.http_client(http_client);
        }

        let sdk_config = config_loader.load().await;

        let mut s3_config = aws_sdk_s3::config::Builder::from(&sdk_config).force_path_style(true);

        if let Some(endpoint) = config.resolve_endpoint(&credentials.region) {
            s3_config = s3_config.endpoint_url(endpoint);
        }

        // When `:log_aws_wire:` is enabled, attach the wire-trace interceptor so
        // every S3 request/response is appended to the restricted wire log. A
        // failure to open the wire log is logged and non-fatal (downloads must
        // still proceed without wire logging).
        if let Some((log_dir, program_name)) = &config.wire_log {
            match super::wire_log::WireLogInterceptor::new(log_dir, program_name) {
                Ok(interceptor) => {
                    info!(
                        "log_aws_wire enabled: S3 wire logs -> {}",
                        super::wire_log::wire_log_path(log_dir, program_name).display()
                    );
                    s3_config = s3_config.interceptor(interceptor);
                },
                Err(e) => {
                    tracing::warn!("Failed to open S3 wire log (log_aws_wire); disabling: {e}");
                },
            }
        }

        // Proxy support: the configured `proxy_uri` is applied above via
        // `build_custom_http_client`, matching the command-control and GitHub
        // clients. The SDK additionally auto-detects the HTTP_PROXY/HTTPS_PROXY
        // env vars when no `proxy_uri` is set.

        Ok(aws_sdk_s3::Client::from_conf(s3_config.build()))
    }

    /// Download an S3 object to a local file.
    ///
    /// Returns the `ETag` of the downloaded object (with quotes stripped).
    ///
    /// # Errors
    /// Returns an error if the download or file write fails.
    pub fn download_to_file(
        &self,
        bucket: &str,
        key: &str,
        version: Option<&str>,
        dest: &Path,
    ) -> io::Result<Option<String>> {
        self.runtime.block_on(self.download_to_file_async(bucket, key, version, dest))
    }

    async fn download_to_file_async(
        &self,
        bucket: &str,
        key: &str,
        version: Option<&str>,
        dest: &Path,
    ) -> io::Result<Option<String>> {
        let version_str = version.unwrap_or("none");
        info!(
            "Downloading artifact bundle from bucket '{bucket}' and key '{key}', version '{version_str}'"
        );

        let mut req = self.inner.get_object().bucket(bucket).key(key);
        if let Some(v) = version {
            req = req.version_id(v);
        }

        let resp = req
            .send()
            .await
            .map_err(|e| io::Error::other(format_s3_get_object_error(&e, bucket, key, version)))?;

        let etag = resp.e_tag().map(|e| e.trim_matches('"').to_string());

        let body = resp.body.into_async_read();
        stream_to_file(body, dest).await?;

        info!("Download complete from bucket '{bucket}' and key '{key}'");
        Ok(etag)
    }
}

async fn stream_to_file(
    mut reader: impl tokio::io::AsyncRead + Unpin,
    dest: &Path,
) -> io::Result<()> {
    // Downloaded bundles are created 0600 so unprivileged users cannot
    // read the archive while the agent is extracting it.
    let mut file = crate::system::create_file_secure(dest, 0o600)?;
    let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
    loop {
        let n = tokio::io::AsyncReadExt::read(&mut reader, &mut buf).await?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])?;
    }
    file.flush()
}

/// Format an S3 `GetObject` failure naming the bucket/key and surfacing the S3
/// error code.
///
/// Includes the bucket/key and the S3 error code (`AccessDenied` vs
/// `NoSuchKey`), which an operator uses to distinguish 403 from 404. Pulls the
/// code/message from the error metadata; falls back to the full source-error
/// chain for non-service failures (dispatch/timeout/TLS) that carry no response
/// metadata.
fn format_s3_get_object_error<E, R>(
    err: &aws_sdk_s3::error::SdkError<E, R>,
    bucket: &str,
    key: &str,
    version: Option<&str>,
) -> String
where
    E: aws_sdk_s3::error::ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug + Send + Sync + 'static,
{
    use aws_sdk_s3::error::ProvideErrorMetadata;

    let location = match version {
        Some(v) => format!("s3://{bucket}/{key} (version {v})"),
        None => format!("s3://{bucket}/{key}"),
    };

    // S3 error code and service message, when response metadata is present.
    let code = err.code().map(str::to_string);
    let svc_msg = err.message().map(str::to_string);

    let mut detail = String::new();
    if let Some(c) = &code {
        detail.push_str(c);
    }
    if let Some(m) = &svc_msg {
        if !detail.is_empty() {
            detail.push_str(": ");
        }
        detail.push_str(m);
    }

    if detail.is_empty() {
        // No response metadata (dispatch/timeout/TLS): use the SDK's full
        // error-context renderer to surface the underlying cause.
        let chain = aws_smithy_types::error::display::DisplayErrorContext(err).to_string();
        format!("S3 GetObject failed for {location}: {chain}")
    } else {
        format!("S3 GetObject failed for {location}: {detail}")
    }
}

impl AwsClient for S3Client {
    fn region(&self) -> &str {
        &self.credentials.region
    }

    fn credentials(&self) -> &Credentials {
        &self.credentials
    }
}

/// Convert agent credentials to AWS SDK credentials for S3.
/// Returns `None` for `InstanceProfile` mode (let the SDK default chain handle IMDS).
///
/// # Errors
/// Returns an error if `IamSession` credentials cannot be loaded from the file.
///
/// NOTE: This function is retained for backward compatibility but is no longer used
/// by `build_client`, which now uses `FileCredentialProvider` directly for `IamSession`.
#[cfg(test)]
fn to_aws_credentials(
    creds: &Credentials,
) -> Result<Option<aws_credential_types::Credentials>, io::Error> {
    match &creds.mode {
        CredentialMode::IamUser { access_key_id, secret_access_key } => {
            Ok(Some(aws_credential_types::Credentials::new(
                access_key_id,
                secret_access_key,
                None,
                None,
                "codedeploy-s3",
            )))
        },
        CredentialMode::InstanceProfile => Ok(None),
        CredentialMode::IamSession { credentials_file } => {
            use crate::aws_clients::file_credentials;
            let creds = file_credentials::load_credentials_from_file(credentials_file)
                .map_err(|e| io::Error::other(format!("IamSession credentials error: {e}")))?;
            Ok(Some(creds))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_clients::credentials::CredentialMode;
    use std::path::PathBuf;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn test_credentials() -> Credentials {
        Credentials {
            region: "us-east-1".to_string(),
            host_identifier: "test-host".to_string(),
            mode: CredentialMode::InstanceProfile,
        }
    }

    #[test]
    fn client_construction() {
        let client = S3Client::new(test_credentials(), &S3ClientConfig::default()).unwrap();
        assert_eq!(client.region(), "us-east-1");
    }

    #[test]
    fn client_with_fips() {
        let config = S3ClientConfig { use_fips: true, ..Default::default() };
        let client = S3Client::new(test_credentials(), &config).unwrap();
        assert_eq!(client.region(), "us-east-1");
    }

    #[test]
    fn client_with_endpoint_override() {
        let config = S3ClientConfig {
            endpoint_override: Some("https://custom-s3.example.com".into()),
            ..Default::default()
        };
        let client = S3Client::new(test_credentials(), &config).unwrap();
        assert_eq!(client.region(), "us-east-1");
    }

    #[test]
    fn resolve_endpoint_override_takes_priority() {
        let config = S3ClientConfig {
            endpoint_override: Some("https://custom.example.com".into()),
            use_fips: true,
            ..Default::default()
        };
        assert_eq!(config.resolve_endpoint("us-east-1"), Some("https://custom.example.com".into()));
    }

    #[test]
    fn resolve_endpoint_fips() {
        let config = S3ClientConfig { use_fips: true, ..Default::default() };
        assert_eq!(
            config.resolve_endpoint("us-west-2"),
            Some("https://s3-fips.us-west-2.amazonaws.com".into())
        );
    }

    #[test]
    fn resolve_endpoint_default() {
        let config = S3ClientConfig::default();
        assert_eq!(config.resolve_endpoint("us-east-1"), None);
    }

    #[tokio::test]
    async fn stream_to_file_writes_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let dest = dir.path().join("out.bin");
        let data = b"hello world";
        let reader = &data[..];
        stream_to_file(reader, &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[tokio::test]
    async fn stream_to_file_empty_body() {
        let dir = tempfile::TempDir::new().unwrap();
        let dest = dir.path().join("empty.bin");
        let reader: &[u8] = &[];
        stream_to_file(reader, &dest).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap().len(), 0);
    }

    #[tokio::test]
    async fn stream_to_file_bad_path() {
        let reader: &[u8] = b"data";
        let result = stream_to_file(reader, Path::new("/nonexistent/dir/file")).await;
        assert!(result.is_err());
    }

    #[test]
    fn client_credentials_accessor() {
        let client = S3Client::new(test_credentials(), &S3ClientConfig::default()).unwrap();
        assert_eq!(client.credentials().region, "us-east-1");
    }

    #[test]
    fn client_with_iam_user_credentials() {
        let creds = Credentials {
            region: "us-west-2".into(),
            host_identifier: "test-host".into(),
            mode: CredentialMode::IamUser {
                access_key_id: "AKIATEST".into(),
                secret_access_key: "secret".into(),
            },
        };
        let client = S3Client::new(creds, &S3ClientConfig::default()).unwrap();
        assert_eq!(client.credentials().region, "us-west-2");
    }

    #[test]
    fn client_with_iam_session_credentials() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[default]\naws_access_key_id = AKIASESSION\naws_secret_access_key = session_secret\naws_session_token = token123"
        )
        .unwrap();
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "test-host".into(),
            mode: CredentialMode::IamSession { credentials_file: file.path().to_path_buf() },
        };
        let client = S3Client::new(creds, &S3ClientConfig::default()).unwrap();
        assert_eq!(client.credentials().region, "us-east-1");
    }

    #[test]
    fn client_with_iam_session_bad_path_fails() {
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "test-host".into(),
            mode: CredentialMode::IamSession {
                credentials_file: PathBuf::from("/nonexistent/creds"),
            },
        };
        let result = S3Client::new(creds, &S3ClientConfig::default());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("IamSession credentials file not found"),
            "expected IamSession error for bad credentials path"
        );
    }

    #[test]
    fn to_aws_credentials_iam_user() {
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "i-test".into(),
            mode: CredentialMode::IamUser {
                access_key_id: "AKIATEST".into(),
                secret_access_key: "secret".into(),
            },
        };
        let aws_creds = to_aws_credentials(&creds).unwrap();
        assert!(aws_creds.is_some());
        let aws_creds = aws_creds.unwrap();
        assert_eq!(aws_creds.access_key_id(), "AKIATEST");
        assert_eq!(aws_creds.secret_access_key(), "secret");
    }

    #[test]
    fn to_aws_credentials_iam_session() {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[default]\naws_access_key_id = AKIASESSION\naws_secret_access_key = session_secret\naws_session_token = token123"
        )
        .unwrap();
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "i-test".into(),
            mode: CredentialMode::IamSession { credentials_file: file.path().to_path_buf() },
        };
        let aws_creds = to_aws_credentials(&creds).unwrap();
        assert!(aws_creds.is_some());
        let aws_creds = aws_creds.unwrap();
        assert_eq!(aws_creds.access_key_id(), "AKIASESSION");
        assert_eq!(aws_creds.secret_access_key(), "session_secret");
        assert_eq!(aws_creds.session_token(), Some("token123"));
    }

    #[test]
    fn to_aws_credentials_iam_session_bad_path() {
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "i-test".into(),
            mode: CredentialMode::IamSession {
                credentials_file: PathBuf::from("/nonexistent/creds"),
            },
        };
        let result = to_aws_credentials(&creds);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("IamSession credentials error"));
    }

    /// Self-signed test certificate (PEM) for CA cert loading tests.
    const TEST_CA_PEM: &str = "-----BEGIN CERTIFICATE-----\n\
        MIICpTCCAY0CFFcX/0W91OjCkbHpYrJ5zuV7ZeeaMA0GCSqGSIb3DQEBCwUAMA8x\n\
        DTALBgNVBAMMBHRlc3QwHhcNMjYwNDIzMDg1MzIzWhcNMjcwNDIzMDg1MzIzWjAP\n\
        MQ0wCwYDVQQDDAR0ZXN0MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA\n\
        zfV7y/lrwJieC0wadhfU6cJxYdcia+/CqxuQfpOZSvHuMxNeP5SMe87VDzD+YEEd\n\
        NgPr5HpxQ8wREdqQhFSsR3nLNasUXgNUb+3TxlOGRJkWMrkysYLiMhDQRpuwRj0b\n\
        hAcxVe3d+0bQhj44+ieBGhq6daWc/SeWReOuTRrDEch1KeajR8sDYycAkiED/C+9\n\
        QDl+9LnEQ+IAsc5WOJw2fYbRLjYKqsljoPELzVDmiwPVvl9qz4WnHUEU6+EDjxeB\n\
        HdrFwUgZ+79bqW3saPRZ4eLcqOMoe2d1/JWfq6A41lAG6Vf5VV/mvCrYZj76Tei9\n\
        w0gSJxR+KEaLm9CxlIqkGQIDAQABMA0GCSqGSIb3DQEBCwUAA4IBAQBRYUpXMTzc\n\
        GiV0ieqC3E9eS2YlEwC7kCdrKItg/pAYvzmn9WDVS/VJu2ZD019BGV1q+SXkD9vC\n\
        w/UpVyLJ9OWbrGzpHhtqT/t7rAT/KGzbQL4KkazblaqeG2cfDqP3g3iha5VQE/fV\n\
        k2ssJH7+tQAwZLXGX/MWMPSYQMxVTmKuVCr1Q9WiZh02//c2BrNMOHqDN386e2R/\n\
        QDXykeTn8fl9wjJWsbcBk8bzVMBalYI564JOqjSKoYOOudvo9d6D+dB/3+ax1HsX\n\
        7+XOtQHfNzjZE9OE1skVKeDUgWXXaiIAJ8NVZd+x441NkjKk0o/FW9D+W6FPA1TD\n\
        L/mY/NUVkadB\n\
        -----END CERTIFICATE-----\n";

    #[test]
    fn custom_http_client_returns_none_when_env_unset() {
        // No CA dir and no proxy → default SDK client (which still honors env proxy vars).
        assert!(build_custom_http_client(None, None).is_none());
    }

    #[test]
    fn imds_no_proxy_default_addresses() {
        // Without an override, both default IMDS addresses are always excluded.
        let rules = imds_no_proxy_rules(None);
        assert!(rules.contains("169.254.169.254"), "IPv4 IMDS must be in no_proxy: {rules}");
        assert!(rules.contains("[fd00:ec2::254]"), "IPv6 IMDS must be in no_proxy: {rules}");
    }

    #[test]
    fn imds_no_proxy_appends_custom_endpoint_host() {
        // AWS_EC2_METADATA_SERVICE_ENDPOINT=http://host:444 → host:444 bypassed too.
        let rules = imds_no_proxy_rules(Some("http://my-imds.internal:444".to_string()));
        assert!(rules.contains("169.254.169.254"));
        assert!(
            rules.contains("my-imds.internal:444"),
            "custom IMDS host must be bypassed: {rules}"
        );
    }

    #[test]
    fn imds_no_proxy_handles_bare_host_and_ipv6_override() {
        // Value without a scheme is used as-is.
        assert!(imds_no_proxy_rules(Some("10.0.0.5".to_string())).contains("10.0.0.5"));
        // IPv6 literal in the override is kept intact (not split on ':').
        let rules = imds_no_proxy_rules(Some("http://[fd00:ec2::99]/".to_string()));
        assert!(rules.contains("[fd00:ec2::99]"), "IPv6 override must stay intact: {rules}");
    }

    #[test]
    fn custom_http_client_returns_some_with_proxy_only() {
        // A configured proxy_uri alone must produce a custom client, even with no CA dir.
        let result = build_custom_http_client(None, Some("http://127.0.0.1:3128"));
        assert!(result.is_some(), "expected Some when proxy_uri is set");
    }

    #[test]
    fn custom_http_client_returns_some_with_valid_ca_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.pem"), TEST_CA_PEM).unwrap();
        let result = build_custom_http_client(Some(dir.path().to_str().unwrap().to_string()), None);
        assert!(result.is_some(), "expected Some when valid CA dir is set");
    }

    #[test]
    fn custom_http_client_returns_some_with_ca_dir_and_proxy() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("test.pem"), TEST_CA_PEM).unwrap();
        let result = build_custom_http_client(
            Some(dir.path().to_str().unwrap().to_string()),
            Some("http://proxy.internal:8080"),
        );
        assert!(result.is_some(), "expected Some when both CA dir and proxy are set");
    }

    #[test]
    fn custom_http_client_returns_none_for_nonexistent_dir() {
        assert!(build_custom_http_client(Some("/nonexistent/ca/dir".to_string()), None).is_none());
    }

    #[test]
    fn custom_http_client_skips_non_pem_files() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("cert.crt"), TEST_CA_PEM).unwrap();
        std::fs::write(dir.path().join("readme.txt"), "not a cert").unwrap();
        let result = build_custom_http_client(Some(dir.path().to_str().unwrap().to_string()), None);
        assert!(result.is_none());
    }

    #[test]
    fn custom_http_client_returns_none_for_empty_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let result = build_custom_http_client(Some(dir.path().to_str().unwrap().to_string()), None);
        assert!(result.is_none());
    }

    #[cfg(unix)]
    #[test]
    fn custom_http_client_returns_none_for_unreadable_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        // Remove read permission to trigger read_dir error
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = build_custom_http_client(Some(dir.path().to_str().unwrap().to_string()), None);
        // Restore permissions for cleanup
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn custom_http_client_returns_none_for_file_not_dir() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let result =
            build_custom_http_client(Some(file.path().to_str().unwrap().to_string()), None);
        assert!(result.is_none());
    }
}
