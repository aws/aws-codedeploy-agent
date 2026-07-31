//! `AppSpec` parsing errors.
use thiserror::Error;

#[derive(Error, Debug, Clone, PartialEq)]
pub enum ParseError {
    #[error(
        "The deployment failed because the application specification file was empty. Make sure your AppSpec file defines at minimum the 'version' and 'os' properties."
    )]
    EmptyFile,

    #[error(
        "The deployment failed because an invalid version value ({0}) was entered in the application specification file. Make sure your AppSpec file specifies \"0.0\" as the version, and then try again."
    )]
    InvalidVersion(String),

    #[error(
        "The deployment failed because the application specification file specifies an unsupported operating system ({0}). Specify either \"linux\" or \"windows\" in the os section of the AppSpec file, and then try again."
    )]
    UnsupportedOs(String),

    #[error(
        "The deployment failed because an invalid file_exists_behavior value ({0}) was entered in the application specification file. Make sure your AppSpec file specifies one of DISALLOW,OVERWRITE,RETAIN as the file_exists_behavior, and then try again."
    )]
    InvalidFileExistsBehavior(String),

    #[error(
        "The deployment failed because the application specification file specifies a script with no location value. Specify the location in the hooks section of the AppSpec file, and then try again."
    )]
    EmptyScriptLocation,

    #[error(
        "The deployment failed because an invalid timeout value was provided for a script in the application specification file. Make corrections in the hooks section of the AppSpec file, and then try again."
    )]
    InvalidTimeout,

    #[error(
        "The deployment failed because the application specification file specifies a destination file, but no source file. Update the files section of the AppSpec file, and then try again."
    )]
    MissingSource,

    #[error(
        "The deployment failed because the application specification file specifies only a source file ({0}). Add the name of the destination file to the files section of the AppSpec file, and then try again."
    )]
    MissingDestination(String),

    #[error(
        "The deployment failed because the application specification file specifies file permissions, but the deployment is targeting one or more Windows Server instances. Permissions are supported only for Amazon Linux, Ubuntu Server, and Red Hat Enterprise Linux (RHEL) instances. Update the permissions section of the AppSpec file, and then try again."
    )]
    PermissionsOnWindows,

    #[error(
        "The deployment failed because a permission listed in the application specification file has no object value. Update the permissions section of the AppSpec file, and then try again."
    )]
    MissingPermissionObject,

    #[error(
        "The deployment failed because the application specification file specifies a permission for an object type not supported for permissions ({0}). Update the permissions section of the AppSpec file, and then try again."
    )]
    InvalidObjectType(String),

    #[error(
        "The deployment failed because the length of a permissions mode ({0}) in the application specification file is invalid. Permissions modes must be between one and four characters long. Update the permissions section of the AppSpec file, and then try again."
    )]
    InvalidModeLength(String),

    #[error(
        "The deployment failed because the permissions mode ({0}) in the application specification file contains an invalid character ({1}). Update the permissions section of the AppSpec file, and then try again."
    )]
    InvalidModeCharacter(String, char),

    #[error(
        "The deployment failed because the application specification file includes an object ({0}) with an invalid pattern ({1}), such as a pattern for a file applied to a directory. Correct the permissions section of the AppSpec file, and then try again."
    )]
    InvalidFilePattern(String, String),

    #[error(
        "The deployment failed because the except parameter for a pattern in the permissions section ({0:?}) for the object named {1} contains an invalid format. Update the AppSpec file, and then try again."
    )]
    InvalidFileExcept(Vec<String>, String),

    #[error(
        "The deployment failed because of a problem with the acls permission settings in the application specification file. Invalid acl entry ({0})."
    )]
    InvalidAclEntry(String),

    #[error(
        "The deployment failed because of a problem with the acls permission settings in the application specification file. Use mode to set the base acl entry ({0}). Update the permissions section of the AppSpec file, and then try again."
    )]
    BaseAclNeedsName(String),

    #[error(
        "The deployment failed because the access control list (ACL) named {0} in the application specification file contains an invalid character ({1}). Correct the ACL in the hooks section of the AppSpec file, and then try again."
    )]
    InvalidAclCharacter(String, char),

    #[error(
        "The deployment failed because the -d parameter has been specified to apply an acl setting to a file. This parameter is supported for directories only. Update the AppSpec file, and then try again."
    )]
    DefaultAclOnFile,

    #[error(
        "The deployment failed because the application specification file specifies an invalid context type ({0}). Update the permissions section of the AppSpec file, and then try again."
    )]
    InvalidContextType(String),

    #[error(
        "The deployment failed because of a problem with the SELinux range specified ({0}) for the context parameter in the permissions section of the application specification file. Make corrections in the permissions section of the AppSpec file, and then try again."
    )]
    InvalidSeLinuxRange(String),

    #[error("The deployment failed because of a YAML parsing error: {0}")]
    YamlError(String),

    #[error("The deployment failed because of an IO error: {0}")]
    IoError(String),
}

impl From<serde_yaml::Error> for ParseError {
    fn from(err: serde_yaml::Error) -> Self {
        ParseError::YamlError(err.to_string())
    }
}

impl From<std::io::Error> for ParseError {
    fn from(err: std::io::Error) -> Self {
        ParseError::IoError(err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application_specification::AppSpec;

    // error_coverage
    #[test]
    fn invalid_version() {
        let yaml = "version: 1.0\nos: linux\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidVersion(_))));
    }

    #[test]
    fn unsupported_os() {
        let yaml = "version: 0.0\nos: macos\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::UnsupportedOs(_))));
    }

    #[test]
    fn invalid_file_exists_behavior() {
        let yaml = "version: 0.0\nos: linux\nfile_exists_behavior: INVALID\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidFileExistsBehavior(_))));
    }

    #[test]
    fn empty_script_location() {
        let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: \"\"\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::EmptyScriptLocation)));
    }

    #[test]
    fn missing_source() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - destination: /tmp/dest\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::MissingSource)));
    }

    #[test]
    fn missing_destination() {
        let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /tmp/src\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::MissingDestination(_))));
    }

    #[test]
    fn permissions_on_windows() {
        let yaml = "version: 0.0\nos: windows\npermissions:\n  - object: /tmp\n    mode: 0755\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::PermissionsOnWindows)));
    }

    #[test]
    fn missing_permission_object() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - mode: 0755\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::MissingPermissionObject)));
    }

    #[test]
    fn invalid_object_type() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - invalid_type\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidObjectType(_))));
    }

    #[test]
    fn invalid_mode_length() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 12345\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidModeLength(_))));
    }

    #[test]
    fn invalid_mode_character() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 08\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidModeCharacter(_, _))));
    }

    #[test]
    fn invalid_acl_entry() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"invalid\"\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidAclEntry(_))));
    }

    #[test]
    fn base_acl_needs_name() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u::rwx\"\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::BaseAclNeedsName(_))));
    }

    #[test]
    fn invalid_acl_character() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:rwz\"\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidAclCharacter(_, _))));
    }

    #[test]
    fn invalid_context_type() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      name: user_u\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidContextType(_))));
    }

    #[test]
    fn invalid_selinux_range() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: invalid\n";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::InvalidSeLinuxRange(_))));
    }

    #[test]
    fn yaml_error() {
        let yaml = "invalid: yaml: content: [";
        assert!(matches!(AppSpec::parse(yaml), Err(ParseError::YamlError(_))));
    }

    // Test Display implementations
    #[test]
    fn error_display() {
        let errors = vec![
            ParseError::EmptyFile,
            ParseError::InvalidVersion("1.0".to_string()),
            ParseError::UnsupportedOs("macos".to_string()),
            ParseError::YamlError("bad yaml".to_string()),
        ];

        for err in errors {
            let display = format!("{err}");
            assert!(!display.is_empty());
        }
    }

    #[test]
    fn error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let parse_err: ParseError = io_err.into();
        assert!(format!("{parse_err}").contains("file not found"));
    }

    // Error messages must match the existing agent for backward compatibility
    // All error messages start with "The deployment failed because..."
    #[test]
    fn error_display_messages() {
        let errors = vec![
            ParseError::EmptyFile,
            ParseError::InvalidFileExistsBehavior("bad".to_string()),
            ParseError::EmptyScriptLocation,
            ParseError::InvalidTimeout,
            ParseError::MissingSource,
            ParseError::MissingDestination("file".to_string()),
            ParseError::PermissionsOnWindows,
            ParseError::MissingPermissionObject,
            ParseError::InvalidObjectType("bad".to_string()),
            ParseError::InvalidModeLength("12345".to_string()),
            ParseError::InvalidModeCharacter("888".to_string(), '8'),
            ParseError::InvalidFilePattern("*.txt".to_string(), "err".to_string()),
            ParseError::InvalidFileExcept(vec!["bad".to_string()], "err".to_string()),
            ParseError::InvalidAclEntry("bad".to_string()),
            ParseError::BaseAclNeedsName("user".to_string()),
            ParseError::InvalidAclCharacter("bad".to_string(), 'x'),
            ParseError::DefaultAclOnFile,
            ParseError::InvalidContextType("bad".to_string()),
            ParseError::InvalidSeLinuxRange("bad".to_string()),
        ];

        // All errors should have meaningful messages starting with "The deployment failed"
        for err in errors {
            let msg = err.to_string();
            assert!(msg.contains("deployment failed"));
            assert!(!msg.is_empty());
        }
    }
}
