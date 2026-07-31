//! Installer error types.
use std::fmt;
use std::path::PathBuf;

#[derive(Debug)]
pub enum InstallerError {
    // Constructor validation errors
    MissingOption(String),

    // Builder conflict errors
    DuplicateCopyTarget {
        source: PathBuf,
        existing_source: PathBuf,
        destination: PathBuf,
    },
    FileMkdirConflict {
        source: PathBuf,
        destination: PathBuf,
    },
    DuplicateMkdir {
        destination: PathBuf,
        existing_source: PathBuf,
    },
    DuplicatePermission {
        object: PathBuf,
    },

    // File exists behavior errors
    FileAlreadyExists {
        destination: PathBuf,
    },
    InvalidFileExistsBehavior {
        behavior: String,
    },
    PathTraversal {
        path: String,
        base: PathBuf,
    },
    DestinationEscapesRoot {
        destination: String,
    },
    InvalidSourceFileName {
        source: String,
    },

    // Permission errors
    SymlinkDestinationRejected {
        object: PathBuf,
    },
    SelinuxRoleNotSupported,
    UnconfinedSelinuxRejected {
        type_: String,
    },
    UnsafePermissionRejected {
        object: PathBuf,
        mode: String,
    },
    AclCommandFailed {
        object: PathBuf,
        command: String,
        exit_code: i32,
    },
    SelinuxCommandFailed {
        command: String,
        exit_code: i32,
    },

    // Parsing errors
    UnknownCommand(String),

    // IO errors
    Io(std::io::Error),
    Json(serde_json::Error),
}

impl fmt::Display for InstallerError {
    // One `write!` arm per error variant; splitting the match would add
    // indirection without improving readability.
    #[allow(clippy::too_many_lines)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Error messages intentionally include full file paths: they surface to
        // the operator (agent log + CodeDeploy console) so they can see which
        // AppSpec path failed. The paths are the customer's own values on a host
        // they control, not a secret.
        match self {
            Self::MissingOption(opt) => write!(f, "the {opt} option is required"),

            Self::DuplicateCopyTarget { source, existing_source, destination } => {
                write!(
                    f,
                    "The deployment failed because the application specification file specifies two source files named {} and {} for the same destination ({}). Remove one of the source file paths from the AppSpec file, and then try again.",
                    source.display(),
                    existing_source.display(),
                    destination.display()
                )
            },

            Self::FileMkdirConflict { source, destination } => {
                write!(
                    f,
                    "The deployment failed because the application specification file calls for installing the file {}, but a file with that name already exists at the location ({}). Update your AppSpec file or directory structure, and then try again.",
                    source.display(),
                    destination.display()
                )
            },

            Self::DuplicateMkdir { destination, existing_source } => {
                write!(
                    f,
                    "The deployment failed because the application specification file includes an mkdir command more than once for the same destination path ({}) from ({}). Update the files section of the AppSpec file, and then try again.",
                    destination.display(),
                    existing_source.display()
                )
            },

            Self::DuplicatePermission { object } => {
                write!(
                    f,
                    "The deployment failed because the permissions setting for ({}) is specified more than once in the application specification file. Update the files section of the AppSpec file, and then try again.",
                    object.display()
                )
            },

            Self::FileAlreadyExists { destination } => {
                write!(
                    f,
                    "The deployment failed because a specified file already exists at this location: {}",
                    destination.display()
                )
            },

            Self::PathTraversal { path, base } => {
                write!(
                    f,
                    "The deployment failed because the source path '{}' attempts to escape the deployment archive directory: {}",
                    path,
                    base.display()
                )
            },

            Self::DestinationEscapesRoot { destination } => {
                write!(
                    f,
                    "The deployment failed because a file destination path ({destination}) uses '..' components that climb above its own root, which would write outside the intended destination. Correct the files section of the AppSpec file, and then try again."
                )
            },

            Self::InvalidSourceFileName { source } => {
                write!(
                    f,
                    "The deployment failed because a file source path ({source}) has no final path component (it ends in '..' or is a root path), so no destination file name can be derived. Correct the files section of the AppSpec file, and then try again."
                )
            },

            Self::InvalidFileExistsBehavior { behavior } => {
                write!(
                    f,
                    "The deployment failed because an invalid option was specified for fileExistsBehavior: {behavior}. Valid options include OVERWRITE, RETAIN, and DISALLOW."
                )
            },

            Self::SymlinkDestinationRejected { object } => {
                write!(
                    f,
                    "The deployment failed because the permission target {} is a symbolic link. The agent does not follow symbolic links when applying ownership, mode, ACL, or SELinux context. Reference the real target path in the AppSpec permissions section, and then try again.",
                    object.display()
                )
            },

            Self::SelinuxRoleNotSupported => {
                write!(
                    f,
                    "The deployment failed because the application specification file specifies a role, but roles are not supported. Remove the role from the AppSpec file, and then try again."
                )
            },

            Self::UnconfinedSelinuxRejected { type_ } => {
                write!(
                    f,
                    "deployment rejected: AppSpec specifies SELinux type '{type_}' which disables mandatory access controls; set reject_unconfined_selinux_in_bundle: false to allow"
                )
            },

            Self::UnsafePermissionRejected { object, mode } => {
                write!(
                    f,
                    "deployment rejected: file {} requests SUID/SGID mode {mode}; set reject_unsafe_permissions_in_bundle: false to allow",
                    object.display()
                )
            },

            Self::AclCommandFailed { object, command, exit_code } => {
                write!(
                    f,
                    "The deployment failed because of a problem with the acls permission settings in the application specification file for this object: {}. Failed command: {}. Exit code: {}",
                    object.display(),
                    command,
                    exit_code
                )
            },

            Self::SelinuxCommandFailed { command, exit_code } => {
                write!(
                    f,
                    "The deployment failed because the application specification file contains an error in the settings for the context parameter. Update the permissions section of the AppSpec file, and then try again. Failed command: {command}. Exit code: {exit_code}"
                )
            },

            Self::UnknownCommand(cmd) => write!(f, "Unknown command: {cmd}"),

            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Json(e) => write!(f, "JSON error: {e}"),
        }
    }
}

impl std::error::Error for InstallerError {}

impl From<std::io::Error> for InstallerError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for InstallerError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<crate::application_specification::ParseError> for InstallerError {
    fn from(e: crate::application_specification::ParseError) -> Self {
        Self::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }
}

pub type Result<T> = std::result::Result<T, InstallerError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_option_display() {
        let err = InstallerError::MissingOption("test_field".to_string());
        assert_eq!(err.to_string(), "the test_field option is required");
    }

    #[test]
    fn duplicate_copy_target_display() {
        let err = InstallerError::DuplicateCopyTarget {
            source: PathBuf::from("/src/file.txt"),
            existing_source: PathBuf::from("/src/other.txt"),
            destination: PathBuf::from("/dest/file.txt"),
        };
        assert!(err.to_string().contains("two source files"));
        assert!(err.to_string().contains("/src/file.txt"));
        assert!(err.to_string().contains("/dest/file.txt"));
    }

    #[test]
    fn file_mkdir_conflict_display() {
        let err = InstallerError::FileMkdirConflict {
            source: PathBuf::from("/src/file.txt"),
            destination: PathBuf::from("/dest/dir"),
        };
        assert!(err.to_string().contains("file with that name already exists"));
    }

    #[test]
    fn duplicate_mkdir_display() {
        let err = InstallerError::DuplicateMkdir {
            destination: PathBuf::from("/dest/dir"),
            existing_source: PathBuf::from("/src/file.txt"),
        };
        assert!(err.to_string().contains("mkdir command more than once"));
    }

    #[test]
    fn duplicate_permission_display() {
        let err = InstallerError::DuplicatePermission { object: PathBuf::from("/path/file.txt") };
        assert!(err.to_string().contains("permissions setting"));
        assert!(err.to_string().contains("/path/file.txt"));
    }

    #[test]
    fn file_already_exists_display() {
        let err =
            InstallerError::FileAlreadyExists { destination: PathBuf::from("/dest/file.txt") };
        assert!(err.to_string().contains("file already exists"));
        assert!(err.to_string().contains("/dest/file.txt"));
    }

    #[test]
    fn invalid_file_exists_behavior_display() {
        let err = InstallerError::InvalidFileExistsBehavior { behavior: "INVALID".to_string() };
        assert!(err.to_string().contains("invalid option"));
        assert!(err.to_string().contains("INVALID"));
    }

    #[test]
    fn path_traversal_display() {
        let err = InstallerError::PathTraversal {
            path: "../etc/passwd".to_string(),
            base: PathBuf::from("/app"),
        };
        assert!(err.to_string().contains("escape the deployment archive"));
        assert!(err.to_string().contains("../etc/passwd"));
    }

    #[test]
    fn destination_escapes_root_display() {
        let err =
            InstallerError::DestinationEscapesRoot { destination: "../../etc/cron.d".to_string() };
        let msg = err.to_string();
        assert!(msg.contains("../../etc/cron.d"));
        assert!(msg.contains("climb above its own root"));
    }

    #[test]
    fn invalid_source_file_name_display() {
        let err = InstallerError::InvalidSourceFileName { source: "foo/..".to_string() };
        let msg = err.to_string();
        assert!(msg.contains("foo/.."));
        assert!(msg.contains("no final path component"));
    }

    #[test]
    fn selinux_role_not_supported_display() {
        let err = InstallerError::SelinuxRoleNotSupported;
        assert!(err.to_string().contains("roles are not supported"));
    }

    #[test]
    fn unconfined_selinux_rejected_display() {
        let err = InstallerError::UnconfinedSelinuxRejected { type_: "unconfined_t".to_string() };
        let msg = err.to_string();
        assert!(msg.contains("unconfined_t"));
        assert!(msg.contains("disables mandatory access controls"));
        assert!(msg.contains("reject_unconfined_selinux_in_bundle"));
    }

    #[test]
    fn unsafe_permission_rejected_display() {
        let err = InstallerError::UnsafePermissionRejected {
            object: PathBuf::from("/path/to/binary"),
            mode: "4755".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("/path/to/binary"));
        assert!(msg.contains("4755"));
        assert!(msg.contains("SUID/SGID"));
        assert!(msg.contains("reject_unsafe_permissions_in_bundle"));
    }

    #[test]
    fn acl_command_failed_display() {
        let err = InstallerError::AclCommandFailed {
            object: PathBuf::from("/path/file.txt"),
            command: "setfacl".to_string(),
            exit_code: 1,
        };
        assert!(err.to_string().contains("acls permission settings"));
        assert!(err.to_string().contains("/path/file.txt"));
        assert!(err.to_string().contains("setfacl"));
    }

    #[test]
    fn selinux_command_failed_display() {
        let err =
            InstallerError::SelinuxCommandFailed { command: "semanage".to_string(), exit_code: 1 };
        assert!(err.to_string().contains("context parameter"));
        assert!(err.to_string().contains("semanage"));
    }

    #[test]
    fn io_error_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let err = InstallerError::Io(io_err);
        assert!(err.to_string().contains("IO error"));
    }

    #[test]
    fn json_error_display() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err = InstallerError::Json(json_err);
        assert!(err.to_string().contains("JSON error"));
    }

    #[test]
    fn unknown_command_display() {
        let err = InstallerError::UnknownCommand("invalid".to_string());
        assert!(err.to_string().contains("Unknown command"));
    }

    #[test]
    fn from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "test");
        let err: InstallerError = io_err.into();
        assert!(matches!(err, InstallerError::Io(_)));
    }

    #[test]
    fn from_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let err: InstallerError = json_err.into();
        assert!(matches!(err, InstallerError::Json(_)));
    }

    #[test]
    fn from_parse_error() {
        use crate::application_specification::ParseError;
        let parse_err = ParseError::InvalidSeLinuxRange("invalid".to_string());
        let err: InstallerError = parse_err.into();
        assert!(matches!(err, InstallerError::Io(_)));
    }
}
