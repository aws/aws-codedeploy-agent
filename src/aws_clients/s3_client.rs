//! S3 client for downloading objects.
//!
//! Wraps `aws-sdk-s3` with agent-specific configuration: endpoint override,
//! FIPS mode, proxy, and signature v4 (SDK default).
//!
//! ## Parallel downloads
//!
//! When `download_max_concurrency` is above 1, a download is split into
//! `download_chunk_size` range requests (default 8 MiB) with up to that many in
//! flight, each written to its own offset of the destination file.
//!
//! - The first request fetches the first chunk and reads the object size from
//!   `Content-Range`, so a small object still costs one request.
//! - Later ranges send `If-Match` with the first `ETag` (and the `versionId`, if
//!   pinned), so an object replaced mid-download fails instead of mixing two
//!   revisions.
//! - Each range retries a body that fails mid-stream, which the SDK does not.
//! - The download falls back to one unranged `GetObject` when the endpoint
//!   refuses ranges, the object is empty, there is no strong `ETag` or version
//!   to pin the object, or a response without `Content-Range` may be truncated.
//!
//! The default concurrency, 1, downloads with one unranged `GetObject`.
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
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, info, warn};

/// Read buffer per streaming body. Peak memory is `concurrency * this`.
const STREAM_BUFFER_SIZE: usize = 1024 * 1024;

/// Default range requests in flight.
pub const DEFAULT_DOWNLOAD_MAX_CONCURRENCY: usize = 1;

/// Default range size.
pub const DEFAULT_DOWNLOAD_CHUNK_SIZE: u64 = 8 * 1024 * 1024;

/// Maximum range requests in flight.
pub const MAX_DOWNLOAD_MAX_CONCURRENCY: usize = 64;

/// Minimum range size.
pub const MIN_DOWNLOAD_CHUNK_SIZE: u64 = 64 * 1024;

/// Maximum download worker threads.
const MAX_DOWNLOAD_WORKER_THREADS: usize = 8;

/// Attempts per range.
const RANGE_ATTEMPTS: u32 = 3;

/// Backoff between range attempts, multiplied by the attempt number.
const RANGE_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_millis(500);

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
#[derive(Debug, Clone)]
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
    /// Range requests in flight per download; `1` means one unranged
    /// `GetObject`. Clamped to `1..=`[`MAX_DOWNLOAD_MAX_CONCURRENCY`].
    pub download_max_concurrency: usize,
    /// Bytes per range request. Floored at [`MIN_DOWNLOAD_CHUNK_SIZE`].
    pub download_chunk_size: u64,
}

impl Default for S3ClientConfig {
    fn default() -> Self {
        Self {
            endpoint_override: None,
            use_fips: false,
            proxy_uri: None,
            wire_log: None,
            download_max_concurrency: DEFAULT_DOWNLOAD_MAX_CONCURRENCY,
            download_chunk_size: DEFAULT_DOWNLOAD_CHUNK_SIZE,
        }
    }
}

impl S3ClientConfig {
    /// Clamped concurrency; `0` becomes `1`.
    fn effective_concurrency(&self) -> usize {
        self.download_max_concurrency.clamp(1, MAX_DOWNLOAD_MAX_CONCURRENCY)
    }

    /// Chunk size floored at [`MIN_DOWNLOAD_CHUNK_SIZE`].
    fn effective_chunk_size(&self) -> u64 {
        self.download_chunk_size.max(MIN_DOWNLOAD_CHUNK_SIZE)
    }

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
    max_concurrency: usize,
    chunk_size: u64,
}

impl S3Client {
    /// Create a new S3 client.
    ///
    /// # Errors
    /// Returns an error if the tokio runtime cannot be created.
    pub fn new(credentials: Credentials, config: &S3ClientConfig) -> io::Result<Self> {
        let max_concurrency = config.effective_concurrency();
        let chunk_size = config.effective_chunk_size();

        // A current-thread runtime would serialize the ranges' TLS decryption
        // and file writes. The workers live as long as the client.
        let runtime = if max_concurrency > 1 {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(download_worker_threads(max_concurrency))
                .thread_name("codedeploy-s3")
                .enable_all()
                .build()?
        } else {
            tokio::runtime::Builder::new_current_thread().enable_all().build()?
        };

        let inner = runtime.block_on(Self::build_client(&credentials, config))?;

        Ok(Self { credentials, inner, runtime, max_concurrency, chunk_size })
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

    /// Download an S3 object to a local file and return its `ETag` (quotes
    /// stripped). Uses parallel range requests unless concurrency is 1.
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

        let etag = if self.max_concurrency == 1 {
            self.download_whole_object(bucket, key, version, dest).await?
        } else {
            match self.download_ranged(bucket, key, version, dest).await {
                Ok(etag) => etag,
                Err(RangedDownloadError::Unsupported(reason)) => {
                    warn!(
                        "Ranged S3 download unavailable ({reason}); retrying as a single request"
                    );
                    self.download_whole_object(bucket, key, version, dest).await?
                },
                Err(RangedDownloadError::Failed(e)) => return Err(e),
            }
        };

        info!("Download complete from bucket '{bucket}' and key '{key}'");
        Ok(etag)
    }

    /// One unranged `GetObject` streamed to disk.
    async fn download_whole_object(
        &self,
        bucket: &str,
        key: &str,
        version: Option<&str>,
        dest: &Path,
    ) -> io::Result<Option<String>> {
        let resp = self
            .inner
            .get_object()
            .bucket(bucket)
            .key(key)
            .set_version_id(version.map(str::to_string))
            .send()
            .await
            .map_err(|e| {
                io::Error::other(format_s3_get_object_error(&e, bucket, key, version, None))
            })?;

        let etag = resp.e_tag().map(|e| e.trim_matches('"').to_string());
        let file = create_download_file(dest)?;
        stream_body_at(&file, resp.body.into_async_read(), 0)
            .await
            .map_err(StreamError::into_io)?;
        Ok(etag)
    }

    /// Download with parallel range requests. The first request fetches the first
    /// chunk and learns the object size from `Content-Range`.
    async fn download_ranged(
        &self,
        bucket: &str,
        key: &str,
        version: Option<&str>,
        dest: &Path,
    ) -> Result<Option<String>, RangedDownloadError> {
        let first = ranged_get(&self.inner, bucket, key, version, None, 0, self.chunk_size)
            .send()
            .await
            .map_err(|e| {
                if range_unsupported(&s3_error_code(&e), http_status(&e)) {
                    RangedDownloadError::Unsupported(s3_error_code(&e))
                } else {
                    RangedDownloadError::Failed(io::Error::other(format_s3_get_object_error(
                        &e,
                        bucket,
                        key,
                        version,
                        Some(0),
                    )))
                }
            })?;

        // The quoted ETag goes back out as If-Match; callers get it unquoted.
        let raw_etag = first.e_tag().map(str::to_string);
        let etag = raw_etag.as_deref().map(|e| e.trim_matches('"').to_string());
        let total = first
            .content_range()
            .and_then(parse_content_range)
            .and_then(|(start, total)| (start == 0).then_some(total));

        let file = create_download_file(dest)?;
        let body = first.body.into_async_read();

        let Some(total) = total else {
            // Without a size there is nothing to plan, so the body must be the
            // whole object or we start over unranged.
            let len = stream_body_at(&file, body, 0).await.map_err(StreamError::into_io)?;
            if first_response_is_complete(len, self.chunk_size) {
                return Ok(etag);
            }
            return Err(RangedDownloadError::Unsupported(
                "response carried no usable Content-Range".to_string(),
            ));
        };

        // We know the size, so a first body that fails mid-stream is re-fetched
        // with the other ranges. A local write failure fails the download.
        let head_len =
            match stream_body_at(&file, tokio::io::AsyncReadExt::take(body, self.chunk_size), 0)
                .await
            {
                Ok(len) => len,
                Err(StreamError::Read(e)) => {
                    warn!(
                        "First range of s3://{bucket}/{key} failed mid-stream ({e}); re-fetching it"
                    );
                    0
                },
                Err(StreamError::Write(e)) => return Err(e.into()),
            };
        if head_len >= total {
            return Ok(etag);
        }

        // Without a strong ETag or a version, nothing stops the object changing
        // between ranges, so download it in one request instead.
        let pin_etag = raw_etag.filter(|e| !e.starts_with("W/"));
        if pin_etag.is_none() && version.is_none() {
            return Err(RangedDownloadError::Unsupported(
                "no strong ETag or version to pin the object".to_string(),
            ));
        }

        let ranges = plan_ranges(head_len, total, self.chunk_size);
        info!(
            "Bundle is {total} bytes: fetching {} ranges of up to {} bytes, {} at a time",
            ranges.len(),
            self.chunk_size,
            self.max_concurrency
        );
        file.set_len(total)?;

        let job = Arc::new(RangeJob {
            client: self.inner.clone(),
            bucket: bucket.to_string(),
            key: key.to_string(),
            version: version.map(str::to_string),
            etag: pin_etag,
            total,
            file,
        });
        self.fetch_ranges(&job, ranges).await?;
        Ok(etag)
    }

    /// Fetch `ranges` with at most `max_concurrency` in flight. On the first
    /// error the other tasks are cancelled and awaited, so none of them is still
    /// writing to the file when this returns.
    async fn fetch_ranges(&self, job: &Arc<RangeJob>, ranges: Vec<(u64, u64)>) -> io::Result<()> {
        let permits = Arc::new(tokio::sync::Semaphore::new(self.max_concurrency));
        let mut tasks = tokio::task::JoinSet::new();
        for (offset, len) in ranges {
            let job = Arc::clone(job);
            let permits = Arc::clone(&permits);
            tasks.spawn(async move {
                let _permit = permits.acquire().await.map_err(io::Error::other)?;
                job.fetch_with_retry(offset, len).await
            });
        }
        while let Some(joined) = tasks.join_next().await {
            if let Err(e) = joined.map_err(io::Error::other).and_then(|r| r) {
                tasks.shutdown().await;
                return Err(e);
            }
        }
        Ok(())
    }
}

/// A ranged `GetObject` request. The SDK's response checksum check is turned
/// off because a full-object checksum header cannot be checked against one
/// range; each range's length is checked instead.
fn ranged_get(
    client: &aws_sdk_s3::Client,
    bucket: &str,
    key: &str,
    version: Option<&str>,
    if_match: Option<&str>,
    offset: u64,
    len: u64,
) -> aws_sdk_s3::client::customize::CustomizableOperation<
    aws_sdk_s3::operation::get_object::GetObjectOutput,
    aws_sdk_s3::operation::get_object::GetObjectError,
    aws_sdk_s3::operation::get_object::builders::GetObjectFluentBuilder,
> {
    client
        .get_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version.map(str::to_string))
        .set_if_match(if_match.map(str::to_string))
        .range(range_header(offset, len))
        .customize()
        .config_override(aws_sdk_s3::config::Builder::default().response_checksum_validation(
            aws_sdk_s3::config::ResponseChecksumValidation::WhenRequired,
        ))
}

/// Why a ranged download did not complete.
enum RangedDownloadError {
    /// The endpoint or object cannot serve ranges; fall back to one request.
    Unsupported(String),
    /// A real failure; the deployment fails.
    Failed(io::Error),
}

impl From<io::Error> for RangedDownloadError {
    fn from(e: io::Error) -> Self {
        Self::Failed(e)
    }
}

/// Worker threads for the download runtime: `min(cores, concurrency)`, capped
/// at [`MAX_DOWNLOAD_WORKER_THREADS`].
fn download_worker_threads(max_concurrency: usize) -> usize {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    cores.min(max_concurrency).clamp(1, MAX_DOWNLOAD_WORKER_THREADS)
}

/// Create the download file with mode 0600.
fn create_download_file(dest: &Path) -> io::Result<File> {
    crate::system::create_file_secure(dest, 0o600)
}

/// `Range` header for `len` (> 0) bytes from `offset`; HTTP ranges are inclusive.
fn range_header(offset: u64, len: u64) -> String {
    debug_assert!(len > 0, "a range must cover at least one byte");
    format!("bytes={offset}-{}", offset + len - 1)
}

/// Split `[start, total)` into `(offset, len)` chunks.
fn plan_ranges(start: u64, total: u64, chunk_size: u64) -> Vec<(u64, u64)> {
    let chunk_size = chunk_size.max(1);
    let mut ranges = Vec::new();
    let mut offset = start;
    while offset < total {
        let len = chunk_size.min(total - offset);
        ranges.push((offset, len));
        offset += len;
    }
    ranges
}

/// `(start, total)` from `Content-Range: bytes 0-99/1234`. `None` if malformed
/// or the total is `*`.
fn parse_content_range(header: &str) -> Option<(u64, u64)> {
    let (range, total) = header.trim().strip_prefix("bytes ")?.split_once('/')?;
    let (start, _) = range.split_once('-')?;
    Some((start.trim().parse().ok()?, total.trim().parse().ok()?))
}

/// Whether a first response without `Content-Range` holds the whole object.
///
/// It normally does (the endpoint ignored `Range` and sent everything). A body of
/// exactly the requested length could also be a truncated range response, so
/// that one case is downloaded again.
fn first_response_is_complete(body_len: u64, requested: u64) -> bool {
    body_len != requested
}

/// Whether a failed first request means the endpoint or object cannot serve
/// ranges: `416` on an empty object, or `501` from endpoints without range
/// support. The status is checked too, because error responses without an XML
/// body carry no code.
fn range_unsupported(code: &str, status: Option<u16>) -> bool {
    matches!(code, "InvalidRange" | "NotImplemented") || matches!(status, Some(416 | 501))
}

/// Whether a failed range request means the object changed (`412`).
fn precondition_failed(code: &str, status: Option<u16>) -> bool {
    code == "PreconditionFailed" || status == Some(412)
}

/// S3 error code, or `"unknown"` when the failure carried none.
fn s3_error_code<E, R>(err: &aws_sdk_s3::error::SdkError<E, R>) -> String
where
    E: aws_sdk_s3::error::ProvideErrorMetadata,
{
    use aws_sdk_s3::error::ProvideErrorMetadata;
    err.code().unwrap_or("unknown").to_string()
}

/// HTTP status of a failed request, when a response was received.
fn http_status<E>(
    err: &aws_sdk_s3::error::SdkError<E, aws_sdk_s3::config::http::HttpResponse>,
) -> Option<u16> {
    err.raw_response().map(|r| r.status().as_u16())
}

/// Everything a range task needs, shared by all ranges of one download.
struct RangeJob {
    client: aws_sdk_s3::Client,
    bucket: String,
    key: String,
    version: Option<String>,
    /// Strong `ETag` as the first response sent it (quoted), sent as `If-Match`.
    etag: Option<String>,
    /// Object size from the first response.
    total: u64,
    file: File,
}

/// A range failure: a body that failed mid-stream can be retried; anything
/// else fails the download.
enum RangeError {
    BodyFailed(io::Error),
    Fatal(io::Error),
}

impl RangeJob {
    /// Fetch one range, retrying a body that fails mid-stream. The SDK already
    /// retries failed requests, so those are not retried again here. Retries are
    /// safe because each range is rewritten at a fixed offset.
    async fn fetch_with_retry(&self, offset: u64, len: u64) -> io::Result<()> {
        let mut attempt = 1;
        loop {
            match self.fetch(offset, len).await {
                Ok(()) => return Ok(()),
                Err(RangeError::BodyFailed(e)) if attempt < RANGE_ATTEMPTS => {
                    warn!(
                        "Range at offset {offset} of s3://{}/{} failed mid-stream (attempt {attempt} of {RANGE_ATTEMPTS}), retrying: {e}",
                        self.bucket, self.key
                    );
                    tokio::time::sleep(RANGE_RETRY_BACKOFF * attempt).await;
                    attempt += 1;
                },
                Err(RangeError::BodyFailed(e) | RangeError::Fatal(e)) => return Err(e),
            }
        }
    }

    async fn fetch(&self, offset: u64, len: u64) -> Result<(), RangeError> {
        let (bucket, key, version) = (&self.bucket, &self.key, self.version.as_deref());
        // versionId and If-Match pin the revision, so an object overwritten
        // between ranges fails instead of producing a mixed file.
        let resp =
            ranged_get(&self.client, bucket, key, version, self.etag.as_deref(), offset, len)
                .send()
                .await
                .map_err(|e| {
                    let mut message =
                        format_s3_get_object_error(&e, bucket, key, version, Some(offset));
                    if precondition_failed(&s3_error_code(&e), http_status(&e)) {
                        message.push_str(" (the object changed during the download)");
                    }
                    RangeError::Fatal(io::Error::other(message))
                })?;

        // The response must be the range we asked for, from the same object.
        let content_range = resp.content_range().and_then(parse_content_range);
        if content_range != Some((offset, self.total)) {
            return Err(RangeError::Fatal(io::Error::other(format!(
                "S3 range at offset {offset} of s3://{bucket}/{key} returned Content-Range {:?}, expected bytes {offset}-{}/{}",
                resp.content_range(),
                offset + len - 1,
                self.total
            ))));
        }

        let body = tokio::io::AsyncReadExt::take(resp.body.into_async_read(), len);
        let written = match stream_body_at(&self.file, body, offset).await {
            Ok(n) => n,
            Err(StreamError::Read(e)) => return Err(RangeError::BodyFailed(e)),
            Err(StreamError::Write(e)) => return Err(RangeError::Fatal(e)),
        };
        if written != len {
            return Err(RangeError::BodyFailed(io::Error::other(format!(
                "S3 range at offset {offset} of s3://{bucket}/{key} returned {written} bytes, expected {len}"
            ))));
        }
        Ok(())
    }
}

/// Which side of [`stream_body_at`] failed.
#[derive(Debug)]
enum StreamError {
    /// Reading the response body (network).
    Read(io::Error),
    /// Writing the file (local disk).
    Write(io::Error),
}

impl StreamError {
    fn into_io(self) -> io::Error {
        match self {
            Self::Read(e) | Self::Write(e) => e,
        }
    }
}

/// Stream `reader` into `file` from `offset` and return the byte count. Uses
/// positioned writes, so ranges can share one file handle.
async fn stream_body_at(
    file: &File,
    mut reader: impl tokio::io::AsyncRead + Unpin,
    offset: u64,
) -> Result<u64, StreamError> {
    let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
    let mut written = 0u64;
    loop {
        let n = tokio::io::AsyncReadExt::read(&mut reader, &mut buf)
            .await
            .map_err(StreamError::Read)?;
        if n == 0 {
            return Ok(written);
        }
        crate::system::write_all_at(file, &buf[..n], offset + written)
            .map_err(StreamError::Write)?;
        written += n as u64;
    }
}

/// Format an S3 `GetObject` failure naming the bucket/key and surfacing the S3
/// error code.
///
/// Includes the bucket/key and the S3 error code (`AccessDenied` vs
/// `NoSuchKey`), which an operator uses to distinguish 403 from 404. Pulls the
/// code/message from the error metadata; falls back to the full source-error
/// chain for non-service failures (dispatch/timeout/TLS) that carry no response
/// metadata.
///
/// `range_offset` is the offset of the failing range, if any.
fn format_s3_get_object_error<E, R>(
    err: &aws_sdk_s3::error::SdkError<E, R>,
    bucket: &str,
    key: &str,
    version: Option<&str>,
    range_offset: Option<u64>,
) -> String
where
    E: aws_sdk_s3::error::ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
    R: std::fmt::Debug + Send + Sync + 'static,
{
    use aws_sdk_s3::error::ProvideErrorMetadata;

    let location = format!(
        "s3://{bucket}/{key}{}{}",
        version.map_or_else(String::new, |v| format!(" (version {v})")),
        range_offset.map_or_else(String::new, |o| format!(" (range at offset {o})")),
    );

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
    async fn stream_body_at_writes_content() {
        let dir = tempfile::TempDir::new().unwrap();
        let dest = dir.path().join("out.bin");
        let data = b"hello world";
        let file = create_download_file(&dest).unwrap();
        let written = stream_body_at(&file, &data[..], 0).await.unwrap();
        assert_eq!(written, data.len() as u64);
        drop(file);
        assert_eq!(std::fs::read(&dest).unwrap(), data);
    }

    #[tokio::test]
    async fn stream_body_at_empty_body() {
        let dir = tempfile::TempDir::new().unwrap();
        let dest = dir.path().join("empty.bin");
        let reader: &[u8] = &[];
        let file = create_download_file(&dest).unwrap();
        assert_eq!(stream_body_at(&file, reader, 0).await.unwrap(), 0);
        drop(file);
        assert_eq!(std::fs::read(&dest).unwrap().len(), 0);
    }

    #[tokio::test]
    async fn stream_body_at_honors_offset() {
        let dir = tempfile::TempDir::new().unwrap();
        let dest = dir.path().join("offset.bin");
        let file = create_download_file(&dest).unwrap();
        file.set_len(6).unwrap();
        stream_body_at(&file, &b"world"[..], 1).await.unwrap();
        stream_body_at(&file, &b"h"[..], 0).await.unwrap();
        drop(file);
        assert_eq!(std::fs::read(&dest).unwrap(), b"hworld");
    }

    #[test]
    fn create_download_file_bad_path() {
        assert!(create_download_file(Path::new("/nonexistent/dir/file")).is_err());
    }

    #[test]
    fn range_header_is_inclusive_end() {
        assert_eq!(range_header(0, 8 * 1024 * 1024), "bytes=0-8388607");
        assert_eq!(range_header(8_388_608, 100), "bytes=8388608-8388707");
    }

    #[test]
    fn plan_ranges_splits_remainder() {
        // 20 bytes total, first 8 already on disk, 8-byte chunks.
        assert_eq!(plan_ranges(8, 20, 8), vec![(8, 8), (16, 4)]);
    }

    #[test]
    fn plan_ranges_exact_multiple() {
        assert_eq!(plan_ranges(10, 30, 10), vec![(10, 10), (20, 10)]);
    }

    #[test]
    fn plan_ranges_nothing_left() {
        assert!(plan_ranges(20, 20, 8).is_empty());
        assert!(plan_ranges(25, 20, 8).is_empty());
    }

    #[test]
    fn plan_ranges_covers_whole_object_without_gaps() {
        let ranges = plan_ranges(0, 1000, 128);
        let mut expected_offset = 0;
        for (offset, len) in &ranges {
            assert_eq!(*offset, expected_offset, "gap or overlap in {ranges:?}");
            expected_offset += len;
        }
        assert_eq!(expected_offset, 1000, "ranges must cover the object exactly");
    }

    #[test]
    fn plan_ranges_zero_chunk_size_does_not_hang() {
        // Defensive: a 0 chunk size would otherwise loop forever.
        assert_eq!(plan_ranges(0, 3, 0), vec![(0, 1), (1, 1), (2, 1)]);
    }

    #[test]
    fn parse_content_range_reads_start_and_total() {
        assert_eq!(parse_content_range("bytes 0-8388607/12345678"), Some((0, 12_345_678)));
        assert_eq!(parse_content_range("bytes 65536-131071/200000"), Some((65_536, 200_000)));
    }

    #[test]
    fn parse_content_range_rejects_unknown_and_malformed() {
        assert_eq!(parse_content_range("bytes 0-99/*"), None);
        assert_eq!(parse_content_range("bytes 0-99"), None);
        assert_eq!(parse_content_range(""), None);
        assert_eq!(parse_content_range("garbage/notanumber"), None);
    }

    #[test]
    fn first_response_completeness() {
        // Shorter than requested → the object is smaller than one chunk.
        assert!(first_response_is_complete(100, 8 * 1024 * 1024));
        // Longer than requested → `Range` was ignored and this is the whole object.
        assert!(first_response_is_complete(32 * 1024 * 1024, 8 * 1024 * 1024));
        // Exactly the requested length → ambiguous, so treated as incomplete.
        assert!(!first_response_is_complete(8 * 1024 * 1024, 8 * 1024 * 1024));
    }

    #[test]
    fn precondition_failed_by_code_or_status() {
        assert!(precondition_failed("PreconditionFailed", None));
        assert!(precondition_failed("unknown", Some(412)));
        assert!(!precondition_failed("AccessDenied", Some(403)));
    }

    /// A `GetObject` service error with S3 response metadata. The raw response
    /// type is `()`, since the helpers only need it to be `Debug + Send + Sync`.
    fn service_error(
        code: &str,
        message: &str,
    ) -> aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::get_object::GetObjectError, ()> {
        let metadata = aws_smithy_types::error::ErrorMetadata::builder()
            .code(code)
            .message(message)
            .build();
        aws_sdk_s3::error::SdkError::service_error(
            aws_sdk_s3::operation::get_object::GetObjectError::generic(metadata),
            (),
        )
    }

    /// A failure with no response metadata, such as a connection or timeout error.
    fn transport_error()
    -> aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::get_object::GetObjectError, ()> {
        aws_sdk_s3::error::SdkError::timeout_error(Box::new(io::Error::other("connect timed out")))
    }

    #[test]
    fn s3_error_code_reads_response_metadata() {
        assert_eq!(s3_error_code(&service_error("AccessDenied", "denied")), "AccessDenied");
    }

    #[test]
    fn s3_error_code_without_metadata_is_unknown() {
        assert_eq!(s3_error_code(&transport_error()), "unknown");
    }

    #[test]
    fn range_unsupported_by_code_or_status() {
        // 416 on an empty object and 501 from endpoints without range support,
        // with or without an XML error body.
        assert!(range_unsupported("InvalidRange", Some(416)));
        assert!(range_unsupported("NotImplemented", Some(501)));
        assert!(range_unsupported("unknown", Some(416)));
        assert!(range_unsupported("unknown", Some(501)));
    }

    #[test]
    fn range_unsupported_does_not_swallow_real_failures() {
        for (code, status) in [
            ("AccessDenied", Some(403)),
            ("NoSuchKey", Some(404)),
            ("SlowDown", Some(503)),
            ("unknown", None),
        ] {
            assert!(!range_unsupported(code, status), "{code} must not fall back");
        }
    }

    #[test]
    fn format_error_names_bucket_key_and_s3_code() {
        let msg = format_s3_get_object_error(
            &service_error("AccessDenied", "Access Denied"),
            "my-bucket",
            "path/to/bundle.zip",
            None,
            None,
        );
        assert!(msg.contains("s3://my-bucket/path/to/bundle.zip"), "{msg}");
        assert!(msg.contains("AccessDenied"), "{msg}");
        assert!(msg.contains("Access Denied"), "{msg}");
    }

    #[test]
    fn format_error_names_version_and_range_offset() {
        let msg = format_s3_get_object_error(
            &service_error("PreconditionFailed", "At least one precondition failed"),
            "my-bucket",
            "bundle.zip",
            Some("v7"),
            Some(8_388_608),
        );
        assert!(msg.contains("version v7"), "{msg}");
        assert!(msg.contains("range at offset 8388608"), "{msg}");
    }

    #[test]
    fn format_error_without_metadata_falls_back_to_the_error_chain() {
        // No S3 code to report, so the underlying cause must still be visible
        // rather than the SDK's opaque top-level "dispatch failure".
        let msg = format_s3_get_object_error(&transport_error(), "bucket", "key", None, Some(1024));
        assert!(msg.contains("s3://bucket/key"), "{msg}");
        assert!(msg.contains("range at offset 1024"), "{msg}");
        assert!(msg.to_lowercase().contains("timeout"), "expected the cause, got: {msg}");
    }

    #[test]
    fn effective_concurrency_is_clamped() {
        let low = S3ClientConfig { download_max_concurrency: 0, ..Default::default() };
        assert_eq!(low.effective_concurrency(), 1, "0 must degrade to serial, not divide by zero");
        let high = S3ClientConfig { download_max_concurrency: 10_000, ..Default::default() };
        assert_eq!(high.effective_concurrency(), MAX_DOWNLOAD_MAX_CONCURRENCY);
        assert_eq!(S3ClientConfig::default().effective_concurrency(), 1);
    }

    #[test]
    fn effective_chunk_size_is_floored() {
        let tiny = S3ClientConfig { download_chunk_size: 1, ..Default::default() };
        assert_eq!(tiny.effective_chunk_size(), MIN_DOWNLOAD_CHUNK_SIZE);
        assert_eq!(S3ClientConfig::default().effective_chunk_size(), 8 * 1024 * 1024);
    }

    #[test]
    fn download_worker_threads_bounded() {
        assert_eq!(download_worker_threads(1), 1);
        assert!(download_worker_threads(64) <= MAX_DOWNLOAD_WORKER_THREADS);
        assert!(download_worker_threads(64) >= 1);
        // Never more threads than ranges we are willing to have in flight.
        assert!(download_worker_threads(2) <= 2);
    }

    #[test]
    fn serial_client_uses_single_threaded_runtime() {
        // Concurrency 1 must not spin up worker threads for the daemon's lifetime.
        let config = S3ClientConfig { download_max_concurrency: 1, ..Default::default() };
        let client = S3Client::new(test_credentials(), &config).unwrap();
        assert_eq!(client.max_concurrency, 1);
        assert_eq!(
            client.runtime.handle().runtime_flavor(),
            tokio::runtime::RuntimeFlavor::CurrentThread
        );
    }

    #[test]
    fn parallel_client_carries_clamped_settings() {
        let config = S3ClientConfig {
            download_max_concurrency: 10_000,
            download_chunk_size: 1,
            ..Default::default()
        };
        let client = S3Client::new(test_credentials(), &config).unwrap();
        assert_eq!(client.max_concurrency, MAX_DOWNLOAD_MAX_CONCURRENCY);
        assert_eq!(client.chunk_size, MIN_DOWNLOAD_CHUNK_SIZE);
        assert_eq!(
            client.runtime.handle().runtime_flavor(),
            tokio::runtime::RuntimeFlavor::MultiThread
        );
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
