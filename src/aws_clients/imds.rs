//! IMDS credential fetching via AWS SDK.
//!
//! Uses the SDK's `ImdsCredentialsProvider` which handles `IMDSv2` token
//! management, credential caching, expiry-based refresh, and retries.

use aws_credential_types::Credentials as AwsCredentials;
use aws_credential_types::provider::ProvideCredentials;
use tracing::debug;

/// Errors from IMDS credential fetching.
#[derive(Debug, thiserror::Error)]
pub enum ImdsError {
    #[error("IMDS credentials unavailable: {0}")]
    Unavailable(String),
}

/// Fetch IAM credentials from IMDS using the AWS SDK.
///
/// Uses `ImdsCredentialsProvider` which:
/// - Handles `IMDSv2` token acquisition and refresh
/// - Fetches IAM role name and credentials automatically
/// - Retries transient failures
///
/// # Errors
/// Returns error if IMDS is unreachable or no IAM role is attached.
// GRCOV_STOP_COVERAGE
pub fn fetch_credentials() -> Result<AwsCredentials, ImdsError> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| ImdsError::Unavailable(e.to_string()))?;

    rt.block_on(async {
        let provider = aws_config::imds::credentials::ImdsCredentialsProvider::builder().build();
        let creds = provider
            .provide_credentials()
            .await
            .map_err(|e| ImdsError::Unavailable(e.to_string()))?;

        debug!(
            "Fetched IMDS credentials (access_key_id={}...)",
            &creds.access_key_id()[..4.min(creds.access_key_id().len())]
        );

        Ok(creds)
    })
}
// GRCOV_BEGIN_COVERAGE

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_credentials_returns_result() {
        // On EC2/dev hosts: returns real credentials.
        // Off EC2: returns error (IMDS unavailable).
        // Either way, must not panic.
        let result = fetch_credentials();
        match &result {
            Ok(creds) => {
                assert!(!creds.access_key_id().is_empty());
                assert!(!creds.secret_access_key().is_empty());
            },
            Err(e) => {
                assert!(e.to_string().contains("unavailable") || e.to_string().contains("IMDS"));
            },
        }
    }

    #[test]
    fn error_display() {
        let e = ImdsError::Unavailable("timeout".into());
        assert!(e.to_string().contains("timeout"));
    }
}
