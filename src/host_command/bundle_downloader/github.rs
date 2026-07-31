//! GitHub bundle downloader.
//!
//! Downloads tarball/zipball from GitHub API with retry logic.
//! Loads custom CA certs from `AWS_SSL_CA_DIRECTORY` env var if set,
//! supporting on-prem environments with custom root CAs.

use super::BundleDownloader;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use tracing::{debug, error, info};

/// Retry delays: 10s, 30s, 90s (exponential backoff: 10 * 3^retries).
const RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(10),
    Duration::from_secs(30),
    Duration::from_secs(90),
];

/// Streaming buffer size — 8 MiB chunks.
const STREAM_BUFFER_SIZE: usize = 8 * 1024 * 1024;

/// `User-Agent` sent on GitHub API requests. GitHub's REST API requires one and
/// returns `403 Forbidden` without it; `reqwest::blocking` sets none by default.
const GITHUB_USER_AGENT: &str = concat!("codedeploy-agent/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleFormat {
    Tar,
    Zip,
}

impl BundleFormat {
    /// Parse from `bundle_type` string. Defaults to Tar on Unix, Zip on Windows
    /// (checks if appspec targets Windows).
    ///
    /// # Errors
    /// Returns an error if the bundle type is not "zip" or "tar".
    pub fn from_bundle_type(bundle_type: Option<&str>) -> io::Result<Self> {
        match bundle_type {
            Some("zip") => Ok(Self::Zip),
            Some("tar") => Ok(Self::Tar),
            None => {
                if cfg!(windows) {
                    Ok(Self::Zip) // GRCOV_IGNORE_LINE
                } else {
                    Ok(Self::Tar)
                }
            },
            Some(other) => Err(io::Error::other(format!(
                "GitHub revision specified with bundle_type other than zip or tar [bundle_type={other}]"
            ))),
        }
    }

    fn api_path(self) -> &'static str {
        match self {
            Self::Tar => "tarball",
            Self::Zip => "zipball",
        }
    }
}

/// Authentication for GitHub API requests.
#[derive(Debug)]
enum GitHubAuth {
    Anonymous,
    Token(String),
}

#[derive(Debug)]
pub struct GitHubDownloader {
    account: String,
    repository: String,
    commit_id: String,
    auth: GitHubAuth,
    format: BundleFormat,
    dest: PathBuf,
    /// HTTP proxy URI, if the agent is configured to route egress through a
    /// proxy. Applied to the download client so GitHub fetches honor the same
    /// `proxy_uri` as S3 and the `CodeDeploy` control-plane clients.
    proxy_uri: Option<String>,
}

impl GitHubDownloader {
    /// Create a new downloader for an anonymous GitHub request.
    #[must_use]
    pub fn anonymous(
        account: String,
        repository: String,
        commit_id: String,
        format: BundleFormat,
        dest: PathBuf,
        proxy_uri: Option<String>,
    ) -> Self {
        Self {
            account,
            repository,
            commit_id,
            auth: GitHubAuth::Anonymous,
            format,
            dest,
            proxy_uri,
        }
    }

    /// Create a new downloader for an authenticated GitHub request.
    #[must_use]
    pub fn authenticated(
        account: String,
        repository: String,
        commit_id: String,
        token: String,
        format: BundleFormat,
        dest: PathBuf,
        proxy_uri: Option<String>,
    ) -> Self {
        Self {
            account,
            repository,
            commit_id,
            auth: GitHubAuth::Token(token),
            format,
            dest,
            proxy_uri,
        }
    }

    fn url(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/{}/{}",
            self.account,
            self.repository,
            self.format.api_path(),
            self.commit_id,
        )
    }

    // GRCOV_STOP_COVERAGE
    fn try_download(&self, client: &reqwest::blocking::Client, url: &str) -> io::Result<()> {
        // GitHub rejects requests with no User-Agent (403).
        let mut req = client.get(url).header("User-Agent", GITHUB_USER_AGENT);
        match &self.auth {
            GitHubAuth::Anonymous => debug!("Anonymous GitHub repository download requested."),
            GitHubAuth::Token(token) => {
                debug!("Authenticated GitHub repository download requested.");
                req = req.header("Authorization", format!("token {token}"));
            },
        }

        let mut response = req
            .send()
            .and_then(reqwest::blocking::Response::error_for_status)
            .map_err(|e| io::Error::other(format!("GitHub download request failed: {e}")))?;

        // Downloaded bundle is created 0600 — unprivileged users should
        // not read the archive while the agent is extracting it.
        let mut file = crate::system::create_file_secure(&self.dest, 0o600)?;
        let mut buf = vec![0u8; STREAM_BUFFER_SIZE];
        loop {
            let n = response
                .read(&mut buf)
                .map_err(|e| io::Error::other(format!("Failed to read response body: {e}")))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
        }
        file.flush()?;
        Ok(())
    }
    // GRCOV_BEGIN_COVERAGE
}

/// Build an HTTPS client, loading custom CA certs from `AWS_SSL_CA_DIRECTORY` if
/// set and routing through `proxy_uri` if configured.
fn build_https_client(
    env: &dyn crate::system::EnvOps,
    proxy_uri: Option<&str>,
) -> io::Result<reqwest::blocking::Client> {
    let mut builder = reqwest::blocking::ClientBuilder::new();

    if let Some(proxy) = proxy_uri {
        let p = reqwest::Proxy::all(proxy)
            .map_err(|e| io::Error::other(format!("invalid proxy URI '{proxy}': {e}")))?;
        builder = builder.proxy(p);
    }

    if let Some(ca_dir) = env.get("AWS_SSL_CA_DIRECTORY") {
        let path = std::path::Path::new(&ca_dir);
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                let file_path = entry.path();
                if file_path.extension().and_then(|e| e.to_str()) == Some("pem")
                    && let Ok(pem_data) = std::fs::read(&file_path)
                    && let Ok(cert) = reqwest::Certificate::from_pem(&pem_data)
                {
                    debug!("Loaded CA cert from {}", file_path.display());
                    builder = builder.add_root_certificate(cert);
                }
            }
        }
    }

    builder
        .build()
        .map_err(|e| io::Error::other(format!("Failed to build HTTPS client: {e}")))
}

// GRCOV_STOP_COVERAGE
impl BundleDownloader for GitHubDownloader {
    fn download(&self) -> io::Result<()> {
        let url = self.url();
        let client = build_https_client(&crate::system::SystemEnvOps, self.proxy_uri.as_deref())?;
        let mut errors: Vec<String> = Vec::new();

        // Only retries on HTTP status errors (not connection/DNS errors).
        // We retry all errors since transient network failures are common on EC2.
        for (attempt, delay) in RETRY_DELAYS.iter().enumerate() {
            info!("Requesting URL: '{url}'");
            match self.try_download(&client, &url) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    let msg = e.to_string();
                    error!("Could not download bundle at '{url}': {msg}");
                    errors.push(msg);
                    info!(
                        "Retrying download in {} seconds (attempt {}/{}).",
                        delay.as_secs(),
                        attempt + 1,
                        RETRY_DELAYS.len(),
                    );
                    thread::sleep(*delay);
                },
            }
        }

        // Final attempt after all retries exhausted
        info!("Requesting URL: '{url}'");
        self.try_download(&client, &url).map_err(|e| {
            errors.push(e.to_string());
            io::Error::other(format!(
                "Could not download bundle at '{url}' after {} retries. Server returned codes: {}.",
                RETRY_DELAYS.len(),
                errors.join("; "),
            ))
        })
    }
}
// GRCOV_BEGIN_COVERAGE

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn url_construction_tarball() {
        let dl = GitHubDownloader::anonymous(
            "acme".into(),
            "app".into(),
            "abc123".into(),
            BundleFormat::Tar,
            PathBuf::from("/tmp/out"),
            None,
        );
        assert_eq!(dl.url(), "https://api.github.com/repos/acme/app/tarball/abc123");
    }

    #[test]
    fn url_construction_zipball() {
        let dl = GitHubDownloader::anonymous(
            "acme".into(),
            "app".into(),
            "abc123".into(),
            BundleFormat::Zip,
            PathBuf::from("/tmp/out"),
            None,
        );
        assert_eq!(dl.url(), "https://api.github.com/repos/acme/app/zipball/abc123");
    }

    #[test]
    fn bundle_format_from_tar() {
        assert_eq!(BundleFormat::from_bundle_type(Some("tar")).unwrap(), BundleFormat::Tar);
    }

    #[test]
    fn bundle_format_from_zip() {
        assert_eq!(BundleFormat::from_bundle_type(Some("zip")).unwrap(), BundleFormat::Zip);
    }

    #[test]
    fn bundle_format_default_on_unix() {
        assert_eq!(BundleFormat::from_bundle_type(None).unwrap(), BundleFormat::Tar);
    }

    #[test]
    fn bundle_format_invalid() {
        let err = BundleFormat::from_bundle_type(Some("rar")).unwrap_err();
        assert!(err.to_string().contains("bundle_type other than zip or tar"));
    }

    #[test]
    fn github_user_agent_is_non_empty() {
        // GitHub returns 403 without a User-Agent; constant must be non-empty.
        assert!(GITHUB_USER_AGENT.starts_with("codedeploy-agent/"));
        assert!(GITHUB_USER_AGENT.len() > "codedeploy-agent/".len());
    }

    #[test]
    fn try_download_bad_host_fails() {
        let dir = TempDir::new().unwrap();
        let dl = GitHubDownloader::anonymous(
            "acme".into(),
            "app".into(),
            "sha".into(),
            BundleFormat::Tar,
            dir.path().join("out"),
            None,
        );
        let client = reqwest::blocking::Client::new();
        assert!(dl.try_download(&client, "http://localhost:1/nope").is_err());
    }

    #[test]
    fn build_https_client_without_ca_dir() {
        use crate::system::MockEnvOps;
        let client = build_https_client(&MockEnvOps::default(), None);
        assert!(client.is_ok());
    }

    #[test]
    fn build_https_client_with_ca_dir_containing_pem() {
        use openssl::asn1::Asn1Time;
        use openssl::hash::MessageDigest;
        use openssl::pkey::PKey;
        use openssl::rsa::Rsa;
        use openssl::x509::X509;

        use crate::system::MockEnvOps;
        let dir = TempDir::new().unwrap();

        // Generate a self-signed cert using the openssl crate

        let rsa = Rsa::generate(2048).unwrap();
        let pkey = PKey::from_rsa(rsa).unwrap();
        let mut builder = X509::builder().unwrap();
        builder.set_pubkey(&pkey).unwrap();
        builder.set_not_before(&Asn1Time::days_from_now(0).unwrap()).unwrap();
        builder.set_not_after(&Asn1Time::days_from_now(1).unwrap()).unwrap();
        builder.sign(&pkey, MessageDigest::sha256()).unwrap();
        let cert = builder.build();

        std::fs::write(dir.path().join("test.pem"), cert.to_pem().unwrap()).unwrap();

        let env = MockEnvOps::with("AWS_SSL_CA_DIRECTORY", dir.path().to_str().unwrap());
        let client = build_https_client(&env, None);
        assert!(client.is_ok());
    }

    #[test]
    fn build_https_client_with_ca_dir_no_pem_files() {
        use crate::system::MockEnvOps;
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("readme.txt"), "not a cert").unwrap();

        let env = MockEnvOps::with("AWS_SSL_CA_DIRECTORY", dir.path().to_str().unwrap());
        let client = build_https_client(&env, None);
        assert!(client.is_ok());
    }

    #[test]
    fn build_https_client_with_nonexistent_ca_dir() {
        use crate::system::MockEnvOps;
        let env = MockEnvOps::with("AWS_SSL_CA_DIRECTORY", "/nonexistent/dir");
        let client = build_https_client(&env, None);
        assert!(client.is_ok());
    }

    #[test]
    fn build_https_client_with_valid_proxy() {
        use crate::system::MockEnvOps;
        let client = build_https_client(&MockEnvOps::default(), Some("http://127.0.0.1:3128"));
        assert!(client.is_ok());
    }

    #[test]
    fn build_https_client_with_invalid_proxy_fails() {
        use crate::system::MockEnvOps;
        // A malformed proxy URI must surface as an error rather than silently
        // building a client that bypasses the proxy.
        let client = build_https_client(&MockEnvOps::default(), Some("://"));
        assert!(client.is_err());
        assert!(client.unwrap_err().to_string().contains("invalid proxy URI"));
    }

    #[test]
    fn authenticated_constructor() {
        let dl = GitHubDownloader::authenticated(
            "acme".into(),
            "app".into(),
            "sha".into(),
            "ghp_secret".into(),
            BundleFormat::Tar,
            PathBuf::from("/tmp/out"),
            None,
        );
        assert!(matches!(dl.auth, GitHubAuth::Token(ref t) if t == "ghp_secret"));
    }
}
