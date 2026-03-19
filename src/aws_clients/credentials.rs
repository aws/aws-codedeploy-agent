//! @risk critical
//!
//! Credential loading from YAML files and instance profile.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;
use tracing::info;

#[derive(Debug, Error)]
pub enum CredentialsError {
    #[error("Config file not found: {0}")]
    FileNotFound(PathBuf),

    #[error(
        "The deployment failed because the format of the following on-premises configuration file is invalid: {path}: {detail}"
    )]
    InvalidYaml { path: PathBuf, detail: String },

    #[error("On Premises config cannot contain both 'iam_user_arn' and 'iam_session_arn' keys.")]
    ConflictingAuthModes,

    #[error("'{field}' key is required when '{mode}' is provided.")]
    MissingField { field: String, mode: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CredentialMode {
    IamUser {
        access_key_id: String,
        secret_access_key: String,
    },
    IamSession {
        /// TODO: Wire as SDK credential provider when real SDK clients are implemented.
        /// Sets credentials from a YAML file which
        /// reads and refreshes credentials from this file.
        credentials_file: PathBuf,
    },
    InstanceProfile,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub region: String,
    pub host_identifier: String,
    pub mode: CredentialMode,
}

#[derive(Debug, Deserialize)]
struct OnPremisesConfigFile {
    region: Option<String>,
    iam_user_arn: Option<String>,
    aws_access_key_id: Option<String>,
    aws_secret_access_key: Option<String>,
    iam_session_arn: Option<String>,
    aws_credentials_file: Option<String>,
}

impl Credentials {
    /// Load credentials from on-premises config file.
    /// Returns `InstanceProfile` mode if file doesn't exist or is not readable.
    ///
    /// # Errors
    /// Returns error if file exists but is invalid or has conflicting auth modes.
    pub fn load(path: &PathBuf) -> Result<Self, CredentialsError> {
        let contents = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e)
                if e.kind() == std::io::ErrorKind::NotFound
                    || e.kind() == std::io::ErrorKind::PermissionDenied =>
            {
                info!("On Premises config file does not exist or not readable");
                return Ok(Self {
                    region: String::new(),
                    host_identifier: String::new(),
                    mode: CredentialMode::InstanceProfile,
                });
            },
            Err(e) => return Err(e.into()),
        };
        let config: OnPremisesConfigFile = serde_yaml::from_str(&contents).map_err(|e| {
            CredentialsError::InvalidYaml { path: path.clone(), detail: e.to_string() }
        })?;

        if config.iam_user_arn.is_some() && config.iam_session_arn.is_some() {
            return Err(CredentialsError::ConflictingAuthModes);
        }

        if let Some(arn) = config.iam_user_arn {
            let mode_name = "iam_user_arn";
            let region = config.region.ok_or_else(|| CredentialsError::MissingField {
                field: "region".into(),
                mode: mode_name.into(),
            })?;
            let access_key_id =
                config.aws_access_key_id.ok_or_else(|| CredentialsError::MissingField {
                    field: "aws_access_key_id".into(),
                    mode: mode_name.into(),
                })?;
            let secret_access_key =
                config.aws_secret_access_key.ok_or_else(|| CredentialsError::MissingField {
                    field: "aws_secret_access_key".into(),
                    mode: mode_name.into(),
                })?;

            Ok(Self {
                region,
                host_identifier: arn,
                mode: CredentialMode::IamUser { access_key_id, secret_access_key },
            })
        } else if let Some(arn) = config.iam_session_arn {
            let mode_name = "iam_session_arn";
            let region = config.region.ok_or_else(|| CredentialsError::MissingField {
                field: "region".into(),
                mode: mode_name.into(),
            })?;
            let credentials_file =
                config.aws_credentials_file.ok_or_else(|| CredentialsError::MissingField {
                    field: "aws_credentials_file".into(),
                    mode: mode_name.into(),
                })?;

            Ok(Self {
                region,
                host_identifier: arn,
                mode: CredentialMode::IamSession {
                    credentials_file: PathBuf::from(credentials_file),
                },
            })
        } else {
            Ok(Self {
                region: String::new(),
                host_identifier: String::new(),
                mode: CredentialMode::InstanceProfile,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::NamedTempFile;

    #[test]
    fn load_nonexistent_file_returns_instance_profile() {
        let path = PathBuf::from("/nonexistent/config.yml");
        let creds = Credentials::load(&path).unwrap();
        assert!(matches!(creds.mode, CredentialMode::InstanceProfile));
        assert_eq!(creds.region, "");
        assert_eq!(creds.host_identifier, "");
    }

    #[cfg(unix)]
    #[test]
    fn load_unreadable_file_returns_instance_profile() {
        use std::os::unix::fs::PermissionsExt;
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "region: us-west-2").unwrap();
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o000)).unwrap();

        let creds = Credentials::load(&file.path().to_path_buf()).unwrap();
        assert!(matches!(creds.mode, CredentialMode::InstanceProfile));

        // Restore permissions so tempfile cleanup works
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn load_iam_user_valid() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "region: us-west-2\niam_user_arn: arn:aws:iam::123:user/test\naws_access_key_id: AKIATEST\naws_secret_access_key: secret123"
        )
        .unwrap();

        let creds = Credentials::load(&file.path().to_path_buf()).unwrap();
        assert_eq!(creds.region, "us-west-2");
        assert_eq!(creds.host_identifier, "arn:aws:iam::123:user/test");
        match creds.mode {
            CredentialMode::IamUser { access_key_id, secret_access_key } => {
                assert_eq!(access_key_id, "AKIATEST");
                assert_eq!(secret_access_key, "secret123");
            },
            _ => panic!("Expected IamUser mode"),
        }
    }

    #[test]
    fn load_iam_session_valid() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "region: us-east-1\niam_session_arn: arn:aws:sts::123:assumed-role/test\naws_credentials_file: /path/to/creds"
        )
        .unwrap();

        let creds = Credentials::load(&file.path().to_path_buf()).unwrap();
        assert_eq!(creds.region, "us-east-1");
        assert_eq!(creds.host_identifier, "arn:aws:sts::123:assumed-role/test");
        match creds.mode {
            CredentialMode::IamSession { credentials_file } => {
                assert_eq!(credentials_file, PathBuf::from("/path/to/creds"));
            },
            _ => panic!("Expected IamSession mode"),
        }
    }

    #[test]
    fn load_conflicting_auth_modes() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "iam_user_arn: arn1\niam_session_arn: arn2\nregion: us-west-2").unwrap();

        let result = Credentials::load(&file.path().to_path_buf());
        assert!(matches!(result, Err(CredentialsError::ConflictingAuthModes)));
    }

    #[test]
    fn load_iam_user_missing_region() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "iam_user_arn: arn:aws:iam::123:user/test\naws_access_key_id: AKIATEST\naws_secret_access_key: secret123"
        )
        .unwrap();

        let result = Credentials::load(&file.path().to_path_buf());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("'region'"));
        assert!(err.to_string().contains("'iam_user_arn'"));
    }

    #[test]
    fn load_iam_user_missing_access_key() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "region: us-west-2\niam_user_arn: arn:aws:iam::123:user/test\naws_secret_access_key: secret123"
        )
        .unwrap();

        let result = Credentials::load(&file.path().to_path_buf());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("'aws_access_key_id'"));
        assert!(err.to_string().contains("'iam_user_arn'"));
    }

    #[test]
    fn load_iam_user_missing_secret_key() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(
            file,
            "region: us-west-2\niam_user_arn: arn:aws:iam::123:user/test\naws_access_key_id: AKIATEST"
        )
        .unwrap();

        let result = Credentials::load(&file.path().to_path_buf());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("'aws_secret_access_key'"));
        assert!(err.to_string().contains("'iam_user_arn'"));
    }

    #[test]
    fn load_iam_session_missing_credentials_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "region: us-east-1\niam_session_arn: arn:aws:sts::123:assumed-role/test")
            .unwrap();

        let result = Credentials::load(&file.path().to_path_buf());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("'aws_credentials_file'"));
        assert!(err.to_string().contains("'iam_session_arn'"));
    }

    #[test]
    fn load_iam_session_missing_region() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "iam_session_arn: arn:aws:sts::123:assumed-role/test\naws_credentials_file: /path/to/creds")
            .unwrap();

        let result = Credentials::load(&file.path().to_path_buf());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("'region'"));
        assert!(err.to_string().contains("'iam_session_arn'"));
    }

    #[test]
    fn load_invalid_yaml() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "invalid: yaml: content: [").unwrap();

        let result = Credentials::load(&file.path().to_path_buf());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("on-premises configuration file is invalid"));
        assert!(err.to_string().contains(&file.path().display().to_string()));
    }

    #[test]
    fn load_empty_file_returns_instance_profile() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file).unwrap();

        let creds = Credentials::load(&file.path().to_path_buf()).unwrap();
        assert!(matches!(creds.mode, CredentialMode::InstanceProfile));
    }
}
