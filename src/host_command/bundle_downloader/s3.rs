//! S3 bundle downloader.
//!
//! Delegates to `S3Client::download_to_file` and verifies the etag.

use super::BundleDownloader;
use crate::aws_clients::S3Client;
use std::io;
use std::path::PathBuf;
use tracing::error;

#[derive(Debug)]
pub struct S3Downloader<'a> {
    client: &'a S3Client,
    bucket: String,
    key: String,
    version: Option<String>,
    etag: Option<String>,
    dest: PathBuf,
}

impl<'a> S3Downloader<'a> {
    #[must_use]
    pub fn new(
        client: &'a S3Client,
        bucket: String,
        key: String,
        version: Option<String>,
        etag: Option<String>,
        dest: PathBuf,
    ) -> Self {
        Self { client, bucket, key, version, etag, dest }
    }

    /// Download the object and return its actual `ETag` (quotes stripped) after
    /// verifying it against the expected etag from the spec.
    ///
    /// The spec often carries a null `ETag`, so the observed value is what
    /// `DownloadBundle` persists to expose `BUNDLE_ETAG` to hooks.
    ///
    /// # Errors
    /// Returns an error if the download or etag verification fails.
    pub fn download_returning_etag(&self) -> io::Result<Option<String>> {
        let actual_etag = self.client.download_to_file(
            &self.bucket,
            &self.key,
            self.version.as_deref(),
            &self.dest,
        )?;
        verify_etag(self.etag.as_deref(), actual_etag.as_deref())?;
        Ok(actual_etag)
    }
}

// GRCOV_STOP_COVERAGE
impl BundleDownloader for S3Downloader<'_> {
    fn download(&self) -> io::Result<()> {
        self.download_returning_etag().map(|_| ())
    }
}
// GRCOV_BEGIN_COVERAGE

/// Verify expected etag matches actual, stripping surrounding quotes.
fn verify_etag(expected: Option<&str>, actual: Option<&str>) -> io::Result<()> {
    if let (Some(expected), Some(actual)) = (expected, actual) {
        let expected = expected.trim_matches('"');
        if expected != actual {
            let msg = format!(
                "Expected deployment artifact bundle etag {expected} but was actually {actual}"
            );
            error!("{msg}");
            return Err(io::Error::other(msg));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_fields_stored() {
        let creds = crate::aws_clients::Credentials {
            region: "us-east-1".into(),
            host_identifier: "test".into(),
            mode: crate::aws_clients::CredentialMode::InstanceProfile,
        };
        let client = S3Client::new(creds, &crate::aws_clients::S3ClientConfig::default()).unwrap();
        let dl = S3Downloader::new(
            &client,
            "bucket".into(),
            "key".into(),
            Some("v1".into()),
            Some("\"abc123\"".into()),
            PathBuf::from("/tmp/out"),
        );
        assert_eq!(dl.bucket, "bucket");
        assert_eq!(dl.key, "key");
        assert_eq!(dl.version.as_deref(), Some("v1"));
        assert_eq!(dl.etag.as_deref(), Some("\"abc123\""));
    }

    #[test]
    fn verify_etag_both_none() {
        assert!(verify_etag(None, None).is_ok());
    }

    #[test]
    fn verify_etag_expected_none() {
        assert!(verify_etag(None, Some("abc")).is_ok());
    }

    #[test]
    fn verify_etag_actual_none() {
        assert!(verify_etag(Some("abc"), None).is_ok());
    }

    #[test]
    fn verify_etag_match() {
        assert!(verify_etag(Some("abc"), Some("abc")).is_ok());
    }

    #[test]
    fn verify_etag_match_with_quotes() {
        assert!(verify_etag(Some("\"abc\""), Some("abc")).is_ok());
    }

    #[test]
    fn verify_etag_mismatch() {
        let err = verify_etag(Some("abc"), Some("xyz")).unwrap_err();
        assert!(err.to_string().contains("Expected deployment artifact bundle etag abc"));
        assert!(err.to_string().contains("but was actually xyz"));
    }
}
