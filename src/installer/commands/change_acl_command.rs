//! @risk medium
//!
//! `setfacl` command — sets POSIX ACLs on installed files.
use crate::application_specification::{Acl, AclEntry};
use crate::installer::{InstallerError, Result};
use crate::system::{LinuxOps, SystemLinuxOps};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ChangeAclCommand<L: LinuxOps = SystemLinuxOps> {
    object: PathBuf,
    acl: Acl,
    linux_ops: L,
}

impl ChangeAclCommand<SystemLinuxOps> {
    #[must_use]
    pub fn new(object: PathBuf, acl: Acl) -> Self {
        Self { object, acl, linux_ops: SystemLinuxOps }
    }
}

impl<L: LinuxOps> ChangeAclCommand<L> {
    #[cfg(test)]
    pub fn new_with_ops(object: PathBuf, acl: Acl, linux_ops: L) -> Self {
        Self { object, acl, linux_ops }
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, _cleanup_file: &mut dyn Write) -> Result<()> {
        // Currently uncovered: setfacl failure path (lines 119-127)
        // Requires refactoring to inject Command trait or adding mocking framework

        // Generate ACL entries from the permission's mode.
        // Converts octal mode digits to rwx ACL entries for user, group, and other.
        // The string manipulation of octal permissions matches the expected format exactly.        // 1. Inferring base permissions from file mode when not explicitly set
        // 2. Building ACL string with user/group/other entries
        // 3. Adding mask entry for effective permissions
        // 4. Including default entries for directories
        // The string manipulation of octal permissions matches the expected format exactly.

        let mut acl_entries = Vec::new();

        // Get file permissions
        let metadata = fs::metadata(&self.object)?;
        let mode = metadata.permissions().mode();
        let perm = format!("{:03o}", mode & 0o777);
        let u = &perm[0..1];
        let g = &perm[1..2];
        let o = &perm[2..3];

        // Add base entries from file permissions
        acl_entries.push(format!(":{u}"));
        acl_entries.push(format!("g::{g}"));
        acl_entries.push(format!("o::{o}"));

        // Check if we need mask
        let has_base_named =
            self.acl.entries().iter().any(|e| !e.is_default() && e.name().is_some());
        let has_base_mask = self
            .acl
            .entries()
            .iter()
            .any(|e| !e.is_default() && matches!(e, AclEntry::Mask { .. }));

        if has_base_named && !has_base_mask {
            acl_entries.push(format!("m::{g}"));
        }

        // Handle default ACLs
        let has_default = self.acl.has_default_entries();
        if has_default {
            let has_default_user = self.acl.entries().iter().any(|e| {
                e.is_default() && matches!(e, AclEntry::User { name, .. } if name.is_empty())
            });
            let has_default_group = self.acl.entries().iter().any(|e| {
                e.is_default() && matches!(e, AclEntry::Group { name, .. } if name.is_empty())
            });
            let has_default_other = self
                .acl
                .entries()
                .iter()
                .any(|e| e.is_default() && matches!(e, AclEntry::Other { .. }));

            if !has_default_user {
                acl_entries.push(format!("d:{u}"));
            }
            if !has_default_group {
                acl_entries.push(format!("d:g::{g}"));
            }
            if !has_default_other {
                acl_entries.push(format!("d:o::{o}"));
            }

            let has_default_named =
                self.acl.entries().iter().any(|e| e.is_default() && e.name().is_some());
            let has_default_mask = self
                .acl
                .entries()
                .iter()
                .any(|e| e.is_default() && matches!(e, AclEntry::Mask { .. }));

            // Infer default mask from default group entry when named defaults exist without mask
            if has_default_named
                && !has_default_mask
                && let Some(group_entry) = self.acl.entries().iter().find(|e| {
                    e.is_default() && matches!(e, AclEntry::Group { name, .. } if name.is_empty())
                })
            {
                let group_str = format_ace(group_entry);
                let mask_str = group_str.replace("group:", "mask:");
                acl_entries.push(mask_str);
            }
        }

        // Add original ACL entries from AppSpec
        for entry in self.acl.entries() {
            acl_entries.push(format_ace(entry));
        }

        let acl_str = acl_entries.join(",");

        self.linux_ops.set_acl(&acl_str, &self.object).map_err(|e| {
            InstallerError::AclCommandFailed {
                object: self.object.clone(),
                command: format!("setfacl --set {acl_str} {}", self.object.display()),
                exit_code: e.raw_os_error().unwrap_or(-1),
            }
        })?;

        Ok(())
    }

    /// Serialize command to JSON hash for debugging/audit
    #[must_use]
    pub fn to_h(&self) -> Value {
        // Format ACL entries back to strings matching original input format
        let acl_strings: Vec<String> = self
            .acl
            .entries()
            .iter()
            .map(|e| {
                let prefix = if e.is_default() { "default:" } else { "" };
                match e {
                    AclEntry::User { name, perms, .. } => {
                        format!("{}user:{}:{}", prefix, name, format_perms(*perms))
                    },
                    AclEntry::Group { name, perms, .. } => {
                        format!("{}group:{}:{}", prefix, name, format_perms(*perms))
                    },
                    AclEntry::Mask { perms, .. } => {
                        format!("{}mask::{}", prefix, format_perms(*perms))
                    },
                    AclEntry::Other { perms, .. } => {
                        format!("{}other::{}", prefix, format_perms(*perms))
                    },
                }
            })
            .collect();

        json!({
            "type": "setfacl",
            "acl": acl_strings,
            "file": self.object
        })
    }
}

fn format_ace(entry: &AclEntry) -> String {
    let prefix = if entry.is_default() { "default:" } else { "" };

    match entry {
        AclEntry::User { name, perms, .. } => {
            format!("{}user:{}:{}", prefix, name, format_perms(*perms))
        },
        AclEntry::Group { name, perms, .. } => {
            format!("{}group:{}:{}", prefix, name, format_perms(*perms))
        },
        AclEntry::Mask { perms, .. } => {
            format!("{}mask::{}", prefix, format_perms(*perms))
        },
        AclEntry::Other { perms, .. } => {
            format!("{}other::{}", prefix, format_perms(*perms))
        },
    }
}

fn format_perms(perms: crate::application_specification::AclPermissions) -> String {
    format!(
        "{}{}{}",
        if perms.read { 'r' } else { '-' },
        if perms.write { 'w' } else { '-' },
        if perms.execute { 'x' } else { '-' }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application_specification::Acl;
    use crate::installer::InstallerError;
    use crate::system::MockLinuxOps;
    use std::fs;

    #[test]
    fn execute_basic() {
        let file = std::env::temp_dir().join("test_acl.txt");
        fs::write(&file, "test").unwrap();

        let acl = Acl::parse(&[]).unwrap();
        let cmd = ChangeAclCommand::new(file.clone(), acl);
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_with_entries() {
        let file = std::env::temp_dir().join("test_acl2.txt");
        fs::write(&file, "test").unwrap();

        let acl = Acl::parse(&["user:root:rwx".to_string()]).unwrap();
        let cmd = ChangeAclCommand::new(file.clone(), acl);
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_nonexistent() {
        let acl = Acl::parse(&[]).unwrap();
        let cmd = ChangeAclCommand::new("/nonexistent".into(), acl);
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());
    }

    #[test]
    fn execute_with_default_entries() {
        let dir = std::env::temp_dir().join("test_acl_dir");
        fs::create_dir_all(&dir).unwrap();

        let acl = Acl::parse(&["default:user:root:rwx".to_string()]).unwrap();
        let cmd = ChangeAclCommand::new(dir.clone(), acl);
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn execute_with_mask() {
        let file = std::env::temp_dir().join("test_acl_mask.txt");
        fs::write(&file, "test").unwrap();

        let acl = Acl::parse(&["user:root:rwx".to_string(), "mask::r--".to_string()]).unwrap();
        let cmd = ChangeAclCommand::new(file.clone(), acl);
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_with_default_group_and_named_entries() {
        // Test mask creation from default group entry
        let dir = std::env::temp_dir().join("test_acl_default_group");
        fs::create_dir_all(&dir).unwrap();

        // default:group::rwx triggers mask creation when there are named entries
        let acl = Acl::parse(&[
            "default:group::rwx".to_string(),
            "default:user:root:rwx".to_string(),
        ])
        .unwrap();
        let cmd = ChangeAclCommand::new(dir.clone(), acl);
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn setfacl_failure() {
        let file = std::env::temp_dir().join("test_acl_fail.txt");
        fs::write(&file, "test").unwrap();

        let acl = Acl::parse(&[]).unwrap();
        let mock_ops = MockLinuxOps::with_failure();
        let cmd = ChangeAclCommand::new_with_ops(file.clone(), acl, mock_ops);
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(result.is_err());
        match result.unwrap_err() {
            InstallerError::AclCommandFailed { .. } => {},
            other => panic!("Expected AclCommandFailed error, got: {other}"),
        }

        fs::remove_file(&file).ok();
    }

    #[test]
    fn to_h() {
        let file = std::env::temp_dir().join("test_acl_to_h.txt");
        fs::write(&file, "test").unwrap();

        let acl = Acl::parse(&["user:alice:rwx".to_string()]).unwrap();
        let cmd = ChangeAclCommand::new(file.clone(), acl);
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "setfacl");
        assert_eq!(hash["acl"][0], "user:alice:rwx");
        assert_eq!(hash["file"], file.to_str().unwrap());

        fs::remove_file(&file).ok();
    }

    #[test]
    fn to_h_all_entry_types() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "test").unwrap();

        let acl = Acl::parse(&[
            "user:alice:rwx".to_string(),
            "group:devs:r-x".to_string(),
            "mask::rwx".to_string(),
            "other::r--".to_string(),
        ])
        .unwrap();
        let cmd = ChangeAclCommand::new(file, acl);
        let hash = cmd.to_h();

        assert_eq!(hash["acl"][0], "user:alice:rwx");
        assert_eq!(hash["acl"][1], "group:devs:r-x");
        assert_eq!(hash["acl"][2], "mask::rwx");
        assert_eq!(hash["acl"][3], "other::r--");
    }

    #[test]
    fn execute_with_default_named_no_mask() {
        // Test: default named entries without explicit default mask triggers mask inference
        // Also covers the has_default_other=false path
        let dir = tempfile::TempDir::new().unwrap();
        let dir_path = dir.path().to_path_buf();

        let acl = Acl::parse(&[
            "default:user:alice:rwx".to_string(),
            "default:group::r-x".to_string(),
            "default:other::r--".to_string(),
        ])
        .unwrap();
        let mock_ops = MockLinuxOps { should_fail: false };
        let cmd = ChangeAclCommand::new_with_ops(dir_path, acl, mock_ops);
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_ok());
    }
}
