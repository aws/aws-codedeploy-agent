//! @risk high
//!
//! S3 client for downloading objects.
//!
//! Wraps `aws-sdk-s3` with agent-specific configuration: endpoint override,
//! FIPS mode, proxy, and signature v4 (SDK default).
//!
//! Downloads use a single `GetObject` request with streaming body.
//! For very large bundles, consider switching to `aws-s3-transfer-manager`
//! which splits downloads into parallel range requests for higher throughput.
//!
//! ## Not yet implemented
//!
//! - **Wire logging**: Enables HTTP wire trace via `log_aws_wire` config with
//!   rotating file logger (1GB max, 64MB chunks). The Rust AWS SDK supports wire
//!   logging via `tracing` (set `aws_smithy_http=trace`). Wire this when the
//!   agent config system is integrated.
//! - **Custom CA certs**: TODO: S3 SDK uses rustls with system certs. For on-prem
//!   environments with custom CAs, install them in the system cert store.
//!   The GitHub downloader already supports `AWS_SSL_CA_DIRECTORY` directly.

use super::aws_client::AwsClient;
use super::credentials::{CredentialMode, Credentials};
use std::io::{self, Write};
use std::path::Path;
use tracing::{debug, info};

/// Streaming buffer size for S3 downloads.
const STREAM_BUFFER_SIZE: usize = 8 * 1024 * 1024;

/// Configuration for S3 client construction.
#[derive(Debug, Default)]
pub struct S3ClientConfig {
    /// Custom S3 endpoint URL (overrides default).
    pub endpoint_override: Option<String>,
    /// Use FIPS endpoints (`https://s3-fips.{region}.amazonaws.com`).
    pub use_fips: bool,
    /// HTTP proxy URI.
    pub proxy_uri: Option<String>,
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

        let inner = runtime.block_on(Self::build_client(&credentials, config));

        Ok(Self { credentials, inner, runtime })
    }

    async fn build_client(
        credentials: &Credentials,
        config: &S3ClientConfig,
    ) -> aws_sdk_s3::Client {
        let mut config_loader = aws_config::from_env()
            .region(aws_sdk_s3::config::Region::new(credentials.region.clone()));

        // Inject agent credentials instead of relying on the default chain.
        // Ruby: `installer.rb` uses the same IAM user / instance-profile credentials
        // for S3 downloads as for the CodeDeploy command service.
        if let Some(aws_creds) = to_aws_credentials(credentials) {
            config_loader = config_loader.credentials_provider(aws_creds);
        }

        let sdk_config = config_loader.load().await;

        let mut s3_config = aws_sdk_s3::config::Builder::from(&sdk_config).force_path_style(true);

        if let Some(endpoint) = config.resolve_endpoint(&credentials.region) {
            s3_config = s3_config.endpoint_url(endpoint);
        }

        // Proxy support: aws-sdk-s3 picks up HTTP_PROXY/HTTPS_PROXY env vars
        // automatically. Proxy can be set from config if needed; the caller
        // should set the env var before constructing this client.

        aws_sdk_s3::Client::from_conf(s3_config.build())
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
            .map_err(|e| io::Error::other(format!("S3 GetObject failed: {e}")))?;

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
    let mut file = std::fs::File::create(dest)?;
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
fn to_aws_credentials(creds: &Credentials) -> Option<aws_credential_types::Credentials> {
    match &creds.mode {
        CredentialMode::IamUser { access_key_id, secret_access_key } => {
            Some(aws_credential_types::Credentials::new(
                access_key_id,
                secret_access_key,
                None,
                None,
                "codedeploy-s3",
            ))
        },
        CredentialMode::InstanceProfile => None,
        CredentialMode::IamSession { .. } => {
            tracing::warn!("IamSession credentials not yet supported for S3 client");
            None
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_clients::credentials::CredentialMode;
    use std::path::PathBuf;

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
    fn to_aws_credentials_iam_user() {
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "i-test".into(),
            mode: CredentialMode::IamUser {
                access_key_id: "AKIATEST".into(),
                secret_access_key: "secret".into(),
            },
        };
        let aws_creds = to_aws_credentials(&creds);
        assert!(aws_creds.is_some());
        let aws_creds = aws_creds.unwrap();
        assert_eq!(aws_creds.access_key_id(), "AKIATEST");
        assert_eq!(aws_creds.secret_access_key(), "secret");
    }

    #[test]
    fn to_aws_credentials_iam_session() {
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "i-test".into(),
            mode: CredentialMode::IamSession { credentials_file: PathBuf::from("/tmp/creds.yaml") },
        };
        let aws_creds = to_aws_credentials(&creds);
        assert!(aws_creds.is_none());
    }
}
