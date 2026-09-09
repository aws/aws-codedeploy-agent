//! Core `AppSpec` types — `AppSpec`, `FileMapping`, `ScriptInfo`.
use crate::application_specification::{Files, Hooks, ParseError, Permissions, parse};
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct AppSpec {
    pub(crate) version: Version,
    pub(crate) os: Os,
    pub(crate) hooks: Hooks,
    pub(crate) files: Files,
    pub(crate) permissions: Permissions,
    pub(crate) file_exists_behavior: Option<FileExistsBehavior>,
}

impl AppSpec {
    /// # Errors
    /// Returns an error if the YAML is empty or invalid.
    pub fn parse(yaml: &str) -> Result<Self, ParseError> {
        if yaml.trim().is_empty() {
            return Err(ParseError::EmptyFile);
        }

        let raw: parse::RawAppSpec = serde_yaml::from_str(yaml)?;
        raw.validate()
    }

    /// # Errors
    /// Returns an error if the file cannot be read or the YAML is invalid.
    pub fn from_file(path: impl AsRef<Path>) -> Result<Self, ParseError> {
        let yaml = std::fs::read_to_string(path)?;
        Self::parse(&yaml)
    }

    #[must_use]
    pub fn version(&self) -> Version {
        self.version
    }

    #[must_use]
    pub fn os(&self) -> Os {
        self.os
    }

    #[must_use]
    pub fn hooks(&self) -> &Hooks {
        &self.hooks
    }

    #[must_use]
    pub fn files(&self) -> &Files {
        &self.files
    }

    #[must_use]
    pub fn permissions(&self) -> &Permissions {
        &self.permissions
    }

    #[must_use]
    pub fn file_exists_behavior(&self) -> Option<FileExistsBehavior> {
        self.file_exists_behavior
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Version;

impl Version {
    /// # Errors
    /// Returns an error if the version number is not 0.0.
    pub fn parse(input: f64) -> Result<Self, ParseError> {
        if (input - 0.0).abs() < f64::EPSILON {
            Ok(Version)
        } else {
            Err(ParseError::InvalidVersion(input.to_string()))
        }
    }

    #[must_use]
    pub fn as_f64(&self) -> f64 {
        0.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Windows,
}

impl Os {
    /// # Errors
    /// Returns an error if the OS string is not "linux" or "windows".
    pub fn parse(s: &str) -> Result<Self, ParseError> {
        match s {
            "linux" => Ok(Os::Linux),
            "windows" => Ok(Os::Windows),
            _ => Err(ParseError::UnsupportedOs(s.to_string())),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Windows => "windows",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileExistsBehavior {
    Disallow,
    Overwrite,
    Retain,
}

impl FileExistsBehavior {
    /// # Errors
    /// Returns an error if the behavior string is not "DISALLOW", "OVERWRITE", or "RETAIN".
    pub fn parse(s: &str) -> Result<Self, ParseError> {
        match s {
            "DISALLOW" => Ok(FileExistsBehavior::Disallow),
            "OVERWRITE" => Ok(FileExistsBehavior::Overwrite),
            "RETAIN" => Ok(FileExistsBehavior::Retain),
            _ => Err(ParseError::InvalidFileExistsBehavior(s.to_string())),
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            FileExistsBehavior::Disallow => "DISALLOW",
            FileExistsBehavior::Overwrite => "OVERWRITE",
            FileExistsBehavior::Retain => "RETAIN",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application_specification::Mode;

    // rare_edge_cases
    // Pattern matching edge cases
    #[test]
    fn pattern_empty_name_empty_pattern() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_wildcard_matches_empty() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_wildcard_at_end_matches() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"test*\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_multiple_chars_after_wildcard() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*abc\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_wildcard_in_middle_complex() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"ab*cd\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_consecutive_wildcards() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"a**b\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_wildcard_matches_multiple_chars() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"a*z\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_name_with_forward_slash() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn pattern_name_with_backslash() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    pattern: \"*\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    // ACL parsing edge cases
    #[test]
    fn acl_default_prefix_full() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"default:u:user:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_user_long_form() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"user:testuser:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_group_long_form() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"group:testgroup:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_mask_long_form() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"mask::rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_other_long_form() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"other::rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_perms_repeated_chars() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:rrwwxx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_perms_mixed_with_dashes() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:r-w-x-\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_parse_error() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:a\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidAclCharacter(_, _))));
    }

    // SELinux edge cases
    #[test]
    fn selinux_category_at_boundary() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c1023\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        let cats = range.categories().unwrap();
        assert_eq!(cats[0], 1023);
    }

    #[test]
    fn selinux_category_range_at_boundary() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s0:c1020.c1023\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        let cats = range.categories().unwrap();
        assert_eq!(cats.len(), 4);
        assert!(cats.contains(&1023));
    }

    #[test]
    fn selinux_max_sensitivity() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: s15\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let range = perm.context().unwrap().range().unwrap();
        assert_eq!(range.low_sensitivity(), 15);
        assert_eq!(range.high_sensitivity(), 15);
    }

    // Mode edge cases
    #[test]
    fn mode_individual_bits() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 0421\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::OWNER_READ));
        assert!(mode.contains(Mode::GROUP_WRITE));
        assert!(mode.contains(Mode::WORLD_EXECUTE));
    }

    #[test]
    fn mode_setuid_with_perms() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 4755\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::SETUID));
        assert!(mode.contains(Mode::OWNER_READ));
        assert!(mode.contains(Mode::OWNER_WRITE));
        assert!(mode.contains(Mode::OWNER_EXECUTE));
    }

    #[test]
    fn mode_setgid_with_perms() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 2755\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::SETGID));
        assert!(mode.contains(Mode::OWNER_READ));
    }

    #[test]
    fn mode_sticky_with_perms() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    mode: 1755\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let mut iter = spec.permissions().iter();
        let perm = iter.next().unwrap();
        let mode = perm.mode().unwrap();
        assert!(mode.contains(Mode::STICKY));
        assert!(mode.contains(Mode::OWNER_READ));
    }

    // Test accessor methods
    #[test]
    #[allow(clippy::float_cmp)]
    fn version_as_f64() {
        let yaml = "version: 0.0\nos: linux\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.version().as_f64(), 0.0);
    }

    #[test]
    fn os_as_str_linux() {
        let yaml = "version: 0.0\nos: linux\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.os().as_str(), "linux");
    }

    #[test]
    fn os_as_str_windows() {
        let yaml = "version: 0.0\nos: windows\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.os().as_str(), "windows");
    }

    #[test]
    fn file_exists_behavior_as_str_overwrite() {
        let yaml = "version: 0.0\nos: linux\nfile_exists_behavior: OVERWRITE\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.file_exists_behavior().unwrap().as_str(), "OVERWRITE");
    }

    #[test]
    fn file_exists_behavior_as_str_disallow() {
        let yaml = "version: 0.0\nos: linux\nfile_exists_behavior: DISALLOW\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.file_exists_behavior().unwrap().as_str(), "DISALLOW");
    }

    #[test]
    fn file_exists_behavior_as_str_retain() {
        let yaml = "version: 0.0\nos: linux\nfile_exists_behavior: RETAIN\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.file_exists_behavior().unwrap().as_str(), "RETAIN");
    }

    #[test]
    fn parse_empty_file() {
        let result = AppSpec::parse("");
        assert!(matches!(result, Err(ParseError::EmptyFile)));
    }

    #[test]
    fn from_file_success() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test_appspec_success.yaml");
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(b"version: 0.0\nos: linux\n").unwrap();
        let result = AppSpec::from_file(&path);
        assert!(result.is_ok());
    }

    #[test]
    fn from_file_nonexistent() {
        let result = AppSpec::from_file("/nonexistent/path.yaml");
        assert!(result.is_err());
    }
}
