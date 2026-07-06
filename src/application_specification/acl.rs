//! `AppSpec` ACL (Access Control List) parsing and generation.
use crate::application_specification::ParseError;

#[derive(Debug, Clone, PartialEq)]
pub struct Acl {
    entries: Vec<AclEntry>,
}

impl Acl {
    pub(crate) fn parse(entries: &[String]) -> Result<Self, ParseError> {
        let parsed: Result<Vec<_>, _> = entries.iter().map(|s| AclEntry::parse(s)).collect();
        Ok(Acl { entries: parsed? })
    }

    #[must_use]
    pub fn entries(&self) -> &[AclEntry] {
        &self.entries
    }

    pub(crate) fn has_default_entries(&self) -> bool {
        self.entries.iter().any(AclEntry::is_default)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AclEntry {
    User {
        name: String,
        perms: AclPermissions,
        default: bool,
    },
    Group {
        name: String,
        perms: AclPermissions,
        default: bool,
    },
    Mask {
        perms: AclPermissions,
        default: bool,
    },
    Other {
        perms: AclPermissions,
        default: bool,
    },
}

impl AclEntry {
    /// Parses a POSIX ACL entry string.
    ///
    /// Format: `[default:|d:]type:name:permissions`
    ///
    /// # Examples
    /// - `u:username:rwx` - User ACL with symbolic permissions
    /// - `g:groupname:7` - Group ACL with numeric permissions
    /// - `m::rwx` - Mask ACL (no name)
    /// - `o::r-x` - Other ACL (no name)
    /// - `default:u:username:rwx` - Default user ACL for directories
    /// - `d:g:groupname:6` - Default group ACL (short form)
    pub(crate) fn parse(s: &str) -> Result<Self, ParseError> {
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() < 3 || parts.len() > 4 {
            return Err(ParseError::InvalidAclEntry(s.to_string()));
        }

        let mut idx = 0;
        let mut is_default = false;

        // Check for "default:" or "d:" prefix
        if parts[idx] == "default" || parts[idx] == "d" {
            is_default = true;
            idx += 1;
        }

        if idx >= parts.len() - 2 {
            return Err(ParseError::InvalidAclEntry(s.to_string()));
        }

        let type_str = parts[idx];
        let name_str = parts[idx + 1];
        let perms_str = parts[idx + 2];

        let perms = AclPermissions::parse(perms_str, s)?;

        match type_str {
            "user" | "u" => {
                if !is_default && name_str.is_empty() {
                    return Err(ParseError::BaseAclNeedsName(s.to_string()));
                }
                Ok(AclEntry::User { name: name_str.to_string(), perms, default: is_default })
            },
            "group" | "g" => {
                if !is_default && name_str.is_empty() {
                    return Err(ParseError::BaseAclNeedsName(s.to_string()));
                }
                Ok(AclEntry::Group { name: name_str.to_string(), perms, default: is_default })
            },
            "mask" | "m" => {
                if !name_str.is_empty() {
                    return Err(ParseError::InvalidAclEntry(s.to_string()));
                }
                Ok(AclEntry::Mask { perms, default: is_default })
            },
            "other" | "o" => {
                if !name_str.is_empty() {
                    return Err(ParseError::InvalidAclEntry(s.to_string()));
                }
                Ok(AclEntry::Other { perms, default: is_default })
            },
            _ => Err(ParseError::InvalidAclEntry(s.to_string())),
        }
    }

    /// Returns whether this is a default ACL entry.
    #[must_use]
    pub fn is_default(&self) -> bool {
        match self {
            AclEntry::User { default, .. }
            | AclEntry::Group { default, .. }
            | AclEntry::Mask { default, .. }
            | AclEntry::Other { default, .. } => *default,
        }
    }

    /// Returns the permissions for this ACL entry.
    #[must_use]
    pub fn permissions(&self) -> &AclPermissions {
        match self {
            AclEntry::User { perms, .. }
            | AclEntry::Group { perms, .. }
            | AclEntry::Mask { perms, .. }
            | AclEntry::Other { perms, .. } => perms,
        }
    }

    /// Returns the name for User/Group entries, None for Mask/Other.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match self {
            AclEntry::User { name, .. } | AclEntry::Group { name, .. } => Some(name),
            AclEntry::Mask { .. } | AclEntry::Other { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AclPermissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl AclPermissions {
    /// Parses ACL permissions from either numeric (0-7) or symbolic (rwx) format.
    ///
    /// # Numeric Format
    /// Single digit 0-7 where bits represent: 4=read, 2=write, 1=execute
    /// - `7` = rwx (4+2+1)
    /// - `6` = rw- (4+2)
    /// - `5` = r-x (4+1)
    /// - `0` = --- (no permissions)
    ///
    /// # Symbolic Format
    /// Any combination of 'r', 'w', 'x', and '-' characters
    /// - `rwx` = all permissions
    /// - `r-x` = read and execute
    /// - `---` = no permissions
    /// - Order doesn't matter: `xwr` is valid
    pub(crate) fn parse(s: &str, full_ace: &str) -> Result<Self, ParseError> {
        // Numeric format: single digit 0-7
        if s.len() == 1 && s.chars().next().unwrap().is_ascii_digit() {
            let val =
                s.parse::<u8>().map_err(|_| ParseError::InvalidAclEntry(full_ace.to_string()))?;
            if val > 7 {
                return Err(ParseError::InvalidAclEntry(full_ace.to_string()));
            }
            return Ok(AclPermissions {
                read: (val & 4) != 0,
                write: (val & 2) != 0,
                execute: (val & 1) != 0,
            });
        }

        // Symbolic format: any combination of r, w, x, -
        let mut perms = AclPermissions { read: false, write: false, execute: false };

        for ch in s.chars() {
            match ch {
                'r' => perms.read = true,
                'w' => perms.write = true,
                'x' => perms.execute = true,
                '-' => {},
                _ => return Err(ParseError::InvalidAclCharacter(full_ace.to_string(), ch)),
            }
        }

        Ok(perms)
    }
}

#[cfg(test)]
mod tests {
    use crate::application_specification::{AppSpec, ParseError};

    // acl_parsing_coverage
    #[test]
    fn acl_short_prefix_d() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"d:u:user:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_short_type_u() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_short_type_g() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"g:group:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_short_type_m() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"m::rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_short_type_o() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"o::rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_0() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:0\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_1() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:1\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_2() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:2\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_3() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:3\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_4() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:4\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_5() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:5\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_6() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:6\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_7() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:7\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_octal_over_7() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:8\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidAclEntry(_))));
    }

    #[test]
    fn acl_perms_read_only() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:r--\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_perms_write_only() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:-w-\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_perms_execute_only() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:--x\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_perms_all_dashes() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:---\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_perms_mixed_order() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:xwr\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_default_group_empty_name() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"d:g:group:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        assert_eq!(spec.permissions().iter().count(), 1);
    }

    #[test]
    fn acl_group_base_needs_name() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"g::rwx\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::BaseAclNeedsName(_))));
    }

    #[test]
    fn acl_numeric_permission_boundary() {
        let yaml_valid = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:7\"\n";
        let yaml_invalid = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:8\"\n";
        assert!(AppSpec::parse(yaml_valid).is_ok());
        assert!(AppSpec::parse(yaml_invalid).is_err());
    }

    // Test accessor methods
    #[test]
    fn acl_entries_accessor() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:rwx\"\n      - \"g:group:r-x\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        assert_eq!(acl.entries().len(), 2);
    }

    #[test]
    fn acl_entry_is_default() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"default:u:user:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        assert!(acl.entries()[0].is_default());
    }

    #[test]
    fn acl_entry_permissions() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        let perms = acl.entries()[0].permissions();
        assert!(perms.read);
        assert!(perms.write);
        assert!(perms.execute);
    }

    #[test]
    fn acl_has_default_entries() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"default:u:user:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        assert!(acl.has_default_entries());
    }

    #[test]
    fn acl_entry_permissions_group() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"g:group:r--\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        let perms = acl.entries()[0].permissions();
        assert!(perms.read);
        assert!(!perms.write);
        assert!(!perms.execute);
    }

    #[test]
    fn acl_entry_permissions_mask() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"m::r-x\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        let perms = acl.entries()[0].permissions();
        assert!(perms.read);
        assert!(!perms.write);
        assert!(perms.execute);
    }

    #[test]
    fn acl_entry_permissions_other() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"o::rw-\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        let perms = acl.entries()[0].permissions();
        assert!(perms.read);
        assert!(perms.write);
        assert!(!perms.execute);
    }

    #[test]
    fn acl_invalid_entry_too_few_parts() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:user\"\n";
        let result = AppSpec::parse(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn acl_invalid_entry_only_type() {
        let yaml =
            "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u::\"\n";
        let result = AppSpec::parse(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn acl_invalid_entry_default_missing_perms() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    type:\n      - directory\n    acls:\n      - \"default:u:user\"\n";
        let result = AppSpec::parse(yaml);
        assert!(matches!(result, Err(ParseError::InvalidAclEntry(_))));
    }

    #[test]
    fn acl_entry_name_user() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"u:testuser:rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        assert_eq!(acl.entries()[0].name(), Some("testuser"));
    }

    #[test]
    fn acl_entry_name_mask() {
        let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    acls:\n      - \"m::rwx\"\n";
        let spec = AppSpec::parse(yaml).unwrap();
        let perm = spec.permissions().iter().next().unwrap();
        let acl = perm.acls().unwrap();
        assert_eq!(acl.entries()[0].name(), None);
    }
}
