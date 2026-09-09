//! INI-format credential file reader for `IamSession` mode.
//!
//! Reads AWS credentials from a shared credentials file (INI format with
//! `[default]` profile) containing `aws_access_key_id`, `aws_secret_access_key`,
//! and optionally `aws_session_token`.
//!
//! The file is re-read on each call to support credential refresh — STS
//! externally updates the file, and the agent picks up fresh credentials
//! on the next client construction.

use crate::aws_clients::credentials::CREDENTIAL_EXPIRATION;
use aws_credential_types::Credentials as AwsCredentials;
use aws_credential_types::provider::{self, ProvideCredentials, future};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use thiserror::Error;
use tracing::debug;

/// Auto-refreshing credential provider that re-reads an INI credentials file.
///
/// Wraps `load_credentials_from_file` with a 30-minute expiry and 5-minute
/// proactive refresh buffer. Re-reads the file from disk on each refresh so
/// externally rotated credentials (e.g., STS token renewal) are picked up
/// automatically.
///
/// The AWS SDK calls `provide_credentials()` before each request and handles
/// caching/refresh internally based on the `Expiry` returned.
#[derive(Debug, Clone)]
pub struct FileCredentialProvider {
    path: PathBuf,
}

impl FileCredentialProvider {
    /// Create a new provider that reads credentials from the given INI file.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl ProvideCredentials for FileCredentialProvider {
    fn provide_credentials<'a>(&'a self) -> future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        let path = self.path.clone();
        future::ProvideCredentials::new(async move {
            let creds = load_credentials_from_file(&path)
                .map_err(provider::error::CredentialsError::not_loaded)?;
            // Set expiration to 30 minutes from now.
            // The SDK will call us again when credentials approach expiry.
            let expiry = SystemTime::now() + CREDENTIAL_EXPIRATION;
            Ok(AwsCredentials::new(
                creds.access_key_id(),
                creds.secret_access_key(),
                creds.session_token().map(String::from),
                Some(expiry),
                "codedeploy-file-credentials",
            ))
        })
    }
}

/// Errors from reading the credentials file.
#[derive(Debug, Error)]
pub enum FileCredentialsError {
    #[error("Credentials file not found: {0}")]
    FileNotFound(String),

    #[error("Failed to read credentials file '{path}': {detail}")]
    ReadError { path: String, detail: String },

    #[error("Missing required field '{field}' in credentials file '{path}'")]
    MissingField { field: String, path: String },

    #[error("No [default] profile found in credentials file '{0}'")]
    NoDefaultProfile(String),
}

/// Read AWS credentials from an INI-format shared credentials file.
///
/// Parses the `[default]` profile and extracts:
/// - `aws_access_key_id` (required)
/// - `aws_secret_access_key` (required)
/// - `aws_session_token` (optional)
///
/// # Errors
/// Returns error if the file cannot be read, has no `[default]` profile,
/// or is missing required fields.
pub fn load_credentials_from_file(path: &Path) -> Result<AwsCredentials, FileCredentialsError> {
    let path_str = path.display().to_string();

    let contents = std::fs::read_to_string(path).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            FileCredentialsError::FileNotFound(path_str.clone())
        } else {
            FileCredentialsError::ReadError { path: path_str.clone(), detail: e.to_string() }
        }
    })?;

    let profiles = parse_ini(&contents);

    let default_profile = profiles
        .get("default")
        .ok_or_else(|| FileCredentialsError::NoDefaultProfile(path_str.clone()))?;

    let access_key_id = default_profile
        .get("aws_access_key_id")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| FileCredentialsError::MissingField {
            field: "aws_access_key_id".into(),
            path: path_str.clone(),
        })?;

    let secret_access_key = default_profile
        .get("aws_secret_access_key")
        .filter(|v| !v.is_empty())
        .ok_or_else(|| FileCredentialsError::MissingField {
            field: "aws_secret_access_key".into(),
            path: path_str.clone(),
        })?;

    let session_token = default_profile.get("aws_session_token").filter(|v| !v.is_empty()).cloned();

    debug!(
        "Loaded credentials from file '{path_str}' (session_token={})",
        session_token.is_some()
    );

    Ok(AwsCredentials::new(
        access_key_id,
        secret_access_key,
        session_token,
        None,
        "codedeploy-file-credentials",
    ))
}

/// Minimal INI parser for AWS shared credentials files.
///
/// Returns a map of profile name → key/value pairs.
/// Handles `[profile]` section headers, `key = value` lines,
/// and ignores comments (`#`, `;`) and blank lines.
fn parse_ini(contents: &str) -> HashMap<String, HashMap<String, String>> {
    let mut profiles: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current_profile: Option<String> = None;

    for line in contents.lines() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }

        // Section header: [profile_name]
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let name = trimmed[1..trimmed.len() - 1].trim().to_string();
            current_profile = Some(name.clone());
            profiles.entry(name).or_default();
            continue;
        }

        // Key = value pair
        if let Some(ref profile) = current_profile
            && let Some((key, value)) = trimmed.split_once('=')
        {
            let key = key.trim().to_string();
            let value = value.trim().to_string();
            if let Some(section) = profiles.get_mut(profile) {
                section.insert(key, value);
            }
        }
    }

    profiles
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn provider_returns_credentials_with_expiry() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[default]\naws_access_key_id = AKIATEST\naws_secret_access_key = secret\naws_session_token = token"
        )
        .unwrap();

        let provider = FileCredentialProvider::new(file.path().to_path_buf());
        let creds = provider.provide_credentials().await.unwrap();
        assert_eq!(creds.access_key_id(), "AKIATEST");
        assert_eq!(creds.secret_access_key(), "secret");
        assert_eq!(creds.session_token(), Some("token"));
        // Credentials should have an expiry set (30 min from now)
        assert!(creds.expiry().is_some());
    }

    #[tokio::test]
    async fn provider_rereads_file_on_each_call() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("credentials");
        std::fs::write(
            &path,
            "[default]\naws_access_key_id = KEY1\naws_secret_access_key = SECRET1",
        )
        .unwrap();

        let provider = FileCredentialProvider::new(path.clone());
        let creds1 = provider.provide_credentials().await.unwrap();
        assert_eq!(creds1.access_key_id(), "KEY1");

        // Simulate STS rotating the file
        std::fs::write(
            &path,
            "[default]\naws_access_key_id = KEY2\naws_secret_access_key = SECRET2",
        )
        .unwrap();

        let creds2 = provider.provide_credentials().await.unwrap();
        assert_eq!(creds2.access_key_id(), "KEY2");
    }

    #[tokio::test]
    async fn provider_returns_error_for_missing_file() {
        let provider = FileCredentialProvider::new(PathBuf::from("/nonexistent/creds"));
        let result = provider.provide_credentials().await;
        assert!(result.is_err());
    }

    #[test]
    fn load_valid_credentials_with_session_token() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[default]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY\naws_session_token = FwoGZXIvYXdzEBYaDHtoken"
        )
        .unwrap();

        let creds = load_credentials_from_file(file.path()).unwrap();
        assert_eq!(creds.access_key_id(), "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(creds.secret_access_key(), "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
        assert_eq!(creds.session_token(), Some("FwoGZXIvYXdzEBYaDHtoken"));
    }

    #[test]
    fn load_valid_credentials_without_session_token() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[default]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE\naws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
        )
        .unwrap();

        let creds = load_credentials_from_file(file.path()).unwrap();
        assert_eq!(creds.access_key_id(), "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(creds.secret_access_key(), "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY");
        assert!(creds.session_token().is_none());
    }

    #[test]
    fn load_missing_access_key_id() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[default]\naws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY"
        )
        .unwrap();

        let result = load_credentials_from_file(file.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("aws_access_key_id"));
    }

    #[test]
    fn load_missing_secret_access_key() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "[default]\naws_access_key_id = AKIAIOSFODNN7EXAMPLE").unwrap();

        let result = load_credentials_from_file(file.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("aws_secret_access_key"));
    }

    #[test]
    fn load_file_not_found() {
        let result = load_credentials_from_file(Path::new("/nonexistent/credentials"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FileCredentialsError::FileNotFound(_)));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn load_empty_file() {
        let file = NamedTempFile::new().unwrap();

        let result = load_credentials_from_file(file.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FileCredentialsError::NoDefaultProfile(_)));
    }

    #[test]
    fn load_no_default_profile() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[production]\naws_access_key_id = AKIATEST\naws_secret_access_key = secret"
        )
        .unwrap();

        let result = load_credentials_from_file(file.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FileCredentialsError::NoDefaultProfile(_)));
    }

    #[test]
    fn load_multiple_profiles_uses_default() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[production]\naws_access_key_id = PROD_KEY\naws_secret_access_key = PROD_SECRET\n\n[default]\naws_access_key_id = DEFAULT_KEY\naws_secret_access_key = DEFAULT_SECRET\naws_session_token = DEFAULT_TOKEN"
        )
        .unwrap();

        let creds = load_credentials_from_file(file.path()).unwrap();
        assert_eq!(creds.access_key_id(), "DEFAULT_KEY");
        assert_eq!(creds.secret_access_key(), "DEFAULT_SECRET");
        assert_eq!(creds.session_token(), Some("DEFAULT_TOKEN"));
    }

    #[test]
    fn parse_ini_handles_comments_and_blank_lines() {
        let contents = "# This is a comment\n; Another comment\n\n[default]\naws_access_key_id = AKIATEST\n# inline comment line\naws_secret_access_key = secret\n";
        let profiles = parse_ini(contents);
        let default = profiles.get("default").unwrap();
        assert_eq!(default.get("aws_access_key_id").unwrap(), "AKIATEST");
        assert_eq!(default.get("aws_secret_access_key").unwrap(), "secret");
    }

    #[test]
    fn parse_ini_handles_spaces_around_equals() {
        let contents = "[default]\naws_access_key_id=AKIATEST\naws_secret_access_key =  secret  \n";
        let profiles = parse_ini(contents);
        let default = profiles.get("default").unwrap();
        assert_eq!(default.get("aws_access_key_id").unwrap(), "AKIATEST");
        assert_eq!(default.get("aws_secret_access_key").unwrap(), "secret");
    }

    #[test]
    fn empty_session_token_treated_as_absent() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "[default]\naws_access_key_id = AKIATEST\naws_secret_access_key = secret\naws_session_token ="
        )
        .unwrap();

        let creds = load_credentials_from_file(file.path()).unwrap();
        assert_eq!(creds.access_key_id(), "AKIATEST");
        assert!(creds.session_token().is_none(), "blank session_token should be None");
    }

    #[test]
    fn empty_access_key_id_is_missing_field_error() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "[default]\naws_access_key_id =\naws_secret_access_key = secret").unwrap();

        let result = load_credentials_from_file(file.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FileCredentialsError::MissingField { .. }));
        assert!(err.to_string().contains("aws_access_key_id"));
    }

    #[test]
    fn empty_secret_access_key_is_missing_field_error() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "[default]\naws_access_key_id = AKIATEST\naws_secret_access_key =").unwrap();

        let result = load_credentials_from_file(file.path());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FileCredentialsError::MissingField { .. }));
        assert!(err.to_string().contains("aws_secret_access_key"));
    }

    #[test]
    fn error_display_messages() {
        let e1 = FileCredentialsError::FileNotFound("/tmp/creds".into());
        assert!(e1.to_string().contains("not found"));

        let e2 = FileCredentialsError::ReadError {
            path: "/tmp/creds".into(),
            detail: "permission denied".into(),
        };
        assert!(e2.to_string().contains("permission denied"));

        let e3 = FileCredentialsError::MissingField {
            field: "aws_access_key_id".into(),
            path: "/tmp/creds".into(),
        };
        assert!(e3.to_string().contains("aws_access_key_id"));

        let e4 = FileCredentialsError::NoDefaultProfile("/tmp/creds".into());
        assert!(e4.to_string().contains("[default]"));
    }

    #[cfg(unix)]
    #[test]
    fn load_unreadable_file_returns_read_error() {
        use std::os::unix::fs::PermissionsExt;

        if nix::unistd::Uid::effective().is_root() {
            // Root bypasses DAC permission checks, so the denial this test
            // relies on never happens (e.g. in CI build containers).
            return;
        }
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "[default]\naws_access_key_id = KEY\naws_secret_access_key = SECRET")
            .unwrap();
        // Remove all read permissions
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = load_credentials_from_file(file.path());
        // Restore permissions for cleanup
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, FileCredentialsError::ReadError { .. }));
    }
}
