//! Install command builder — generates copy, permission, and cleanup commands from `AppSpec`.
use crate::application_specification::Permission;
#[cfg(unix)]
use crate::installer::commands::{
    ChangeAclCommand, ChangeContextCommand, ChangeModeCommand, ChangeOwnerCommand,
};
use crate::installer::commands::{CopyCommand, MakeDirectoryCommand, RemoveCommand};
use crate::installer::{InstallerError, Result};
use nu_path;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::debug;

#[derive(Debug)]
pub struct CommandBuilder {
    commands: Vec<Command>,
    copy_targets: HashMap<PathBuf, PathBuf>,
    mkdir_targets: HashSet<PathBuf>,
    permission_targets: HashSet<PathBuf>,
    reject_unconfined_selinux: bool,
    reject_unsafe_permissions: bool,
    reject_symlink_permission_targets: bool,
}

#[derive(Debug)]
pub enum Command {
    Copy(CopyCommand),
    Mkdir(MakeDirectoryCommand),
    Remove(RemoveCommand),
    #[cfg(unix)]
    Chmod(ChangeModeCommand),
    #[cfg(unix)]
    Chown(ChangeOwnerCommand),
    #[cfg(unix)]
    Acl(ChangeAclCommand),
    #[cfg(unix)]
    Context(ChangeContextCommand),
}

impl Command {
    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute_with_cleanup<W: std::io::Write>(&self, cleanup: &mut W) -> Result<()> {
        match self {
            Command::Copy(cmd) => cmd.execute(cleanup),
            Command::Mkdir(cmd) => cmd.execute(cleanup),
            Command::Remove(cmd) => cmd.execute(cleanup),
            #[cfg(unix)]
            Command::Chmod(cmd) => cmd.execute(cleanup),
            #[cfg(unix)]
            Command::Chown(cmd) => cmd.execute(cleanup),
            #[cfg(unix)]
            Command::Acl(cmd) => cmd.execute(cleanup),
            #[cfg(unix)]
            Command::Context(cmd) => cmd.execute(cleanup),
        }
    }

    /// Serialize command to JSON hash for debugging/audit
    #[must_use]
    pub fn to_h(&self) -> Value {
        match self {
            Command::Copy(cmd) => cmd.to_h(),
            Command::Mkdir(cmd) => cmd.to_h(),
            Command::Remove(cmd) => cmd.to_h(),
            #[cfg(unix)]
            Command::Chmod(cmd) => cmd.to_h(),
            #[cfg(unix)]
            Command::Chown(cmd) => cmd.to_h(),
            #[cfg(unix)]
            Command::Acl(cmd) => cmd.to_h(),
            #[cfg(unix)]
            Command::Context(cmd) => cmd.to_h(),
        }
    }
}

impl Default for CommandBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self::with_options(false, false, false)
    }

    #[must_use]
    pub fn with_options(
        reject_unconfined_selinux: bool,
        reject_unsafe_permissions: bool,
        reject_symlink_permission_targets: bool,
    ) -> Self {
        Self {
            commands: Vec::new(),
            copy_targets: HashMap::new(),
            mkdir_targets: HashSet::new(),
            permission_targets: HashSet::new(),
            reject_unconfined_selinux,
            reject_unsafe_permissions,
            reject_symlink_permission_targets,
        }
    }

    /// # Errors
    /// Returns an error if there's a duplicate copy target or file/mkdir conflict.
    //
    // This sink does not validate containment. CodeDeploy has no
    // allowed-directory sandbox — customers deploy to arbitrary absolute
    // destinations, so by default this is pass-through. Containment is enforced
    // upstream in `core.rs::process_file_mapping`, only under the opt-in
    // `reject_path_traversal_in_bundle` flag (source-escapes-archive and
    // destination-escapes-own-root); with the flag off neither fires. The
    // destination-symlink write race is closed independently by the O_NOFOLLOW
    // copy in `copy_command.rs`.
    pub fn copy(&mut self, source: &Path, destination: &Path) -> Result<()> {
        debug!("Copying {} to {}", source.display(), destination.display());

        let destination = Self::expand_path(destination);

        if let Some(existing_source) = self.copy_targets.get(&destination) {
            return Err(InstallerError::DuplicateCopyTarget {
                source: source.to_path_buf(),
                existing_source: existing_source.clone(),
                destination,
            });
        }

        if self.mkdir_targets.contains(&destination) {
            return Err(InstallerError::FileMkdirConflict {
                source: source.to_path_buf(),
                destination,
            });
        }

        self.commands
            .push(Command::Copy(CopyCommand::new(source.to_path_buf(), destination.clone())));
        self.copy_targets.insert(destination, source.to_path_buf());
        Ok(())
    }

    /// # Errors
    /// Returns an error if there's a duplicate mkdir target or file/mkdir conflict.
    pub fn mkdir(&mut self, destination: &Path) -> Result<()> {
        debug!("Making directory {}", destination.display());

        let destination = Self::expand_path(destination);

        if let Some(existing_source) = self.copy_targets.get(&destination) {
            return Err(InstallerError::DuplicateMkdir {
                destination,
                existing_source: existing_source.clone(),
            });
        }

        if !self.mkdir_targets.contains(&destination) {
            self.commands
                .push(Command::Mkdir(MakeDirectoryCommand::new(destination.clone())));
        }
        self.mkdir_targets.insert(destination);
        Ok(())
    }

    /// # Errors
    /// Returns an error if there's a duplicate permission target.
    pub fn set_permissions(&mut self, object: &Path, permission: &Permission) -> Result<()> {
        debug!("Setting permissions on {}", object.display());

        let object = Self::expand_path(object);

        if self.permission_targets.contains(&object) {
            return Err(InstallerError::DuplicatePermission { object });
        }
        self.permission_targets.insert(object.clone());

        #[cfg(not(unix))]
        let _ = permission;

        #[cfg(unix)]
        {
            if let Some(mode) = permission.mode() {
                self.commands.push(Command::Chmod(ChangeModeCommand::new(
                    object.clone(),
                    format!("{:o}", mode.bits()),
                    self.reject_unsafe_permissions,
                    self.reject_symlink_permission_targets,
                )));
            }

            if let Some(acls) = permission.acls() {
                self.commands.push(Command::Acl(ChangeAclCommand::new(
                    object.clone(),
                    acls.clone(),
                    self.reject_symlink_permission_targets,
                )));
            }

            if let Some(context) = permission.context() {
                self.commands.push(Command::Context(ChangeContextCommand::new(
                    object.clone(),
                    context.clone(),
                    self.reject_unconfined_selinux,
                    self.reject_symlink_permission_targets,
                )));
            }

            if permission.owner().is_some() || permission.group().is_some() {
                self.commands.push(Command::Chown(ChangeOwnerCommand::new(
                    object.clone(),
                    permission.owner().map(std::string::ToString::to_string),
                    permission.group().map(std::string::ToString::to_string),
                    self.reject_symlink_permission_targets,
                )));
            }
        }

        Ok(())
    }

    #[must_use]
    pub fn copying_file(&self, file: &Path) -> bool {
        debug!("Checking if copying file {}", file.display());

        let file = Self::expand_path(file);
        self.copy_targets.contains_key(&file)
    }

    #[must_use]
    pub fn making_directory(&self, dir: &Path) -> bool {
        debug!("Checking if making directory {}", dir.display());

        let dir = Self::expand_path(dir);
        self.mkdir_targets.contains(&dir)
    }

    /// # Errors
    /// Returns an error if glob pattern matching fails.
    pub fn find_matches(&self, permission: &Permission) -> Result<Vec<PathBuf>> {
        let mut matches = Vec::new();

        if permission.types().contains(&crate::application_specification::ObjectType::File) {
            for object in self.copy_targets.keys() {
                if permission.matches_pattern(object) && !permission.matches_except(object) {
                    // The directory-object path validates only the ACL per
                    // matched file, never the pattern/except (that validation
                    // fires only on the copying-file path, where `object:`
                    // directly names a copied file). Here `object:` is a
                    // directory whose `pattern:`/`except:` legitimately select
                    // the files under it.
                    permission.validate_file_acl(object)?;
                    matches.push(object.clone());
                }
            }
        }

        if permission
            .types()
            .contains(&crate::application_specification::ObjectType::Directory)
        {
            for object in &self.mkdir_targets {
                if permission.matches_pattern(object) && !permission.matches_except(object) {
                    matches.push(object.clone());
                }
            }
        }

        Ok(matches)
    }

    #[must_use]
    pub fn commands(&self) -> &[Command] {
        &self.commands
    }

    /// Expands and normalizes paths for consistent deduplication tracking.
    ///
    /// Converts relative paths, `~`, `.`, and `..` to absolute normalized form
    /// so different representations of the same path are treated identically.
    ///
    /// Uses `nu_path::expand_path` which resolves `..` and `.` without
    /// requiring file existence.
    ///
    /// `~` is expanded as well. Used only for internal dedup-key normalization,
    /// never as a trust boundary.
    fn expand_path(path: &Path) -> PathBuf {
        nu_path::expand_path(path, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::commands::*;
    use std::fs;

    #[test]
    fn new() {
        let builder = CommandBuilder::new();
        assert_eq!(builder.commands().len(), 0);
    }

    #[test]
    fn default() {
        let builder = CommandBuilder::default();
        assert_eq!(builder.commands().len(), 0);
    }

    #[test]
    fn copy() {
        let mut builder = CommandBuilder::new();
        let src = std::env::temp_dir().join("test_src.txt");
        let dst = std::env::temp_dir().join("test_dst.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst, "test").unwrap();

        builder.copy(&src, &dst).unwrap();
        assert_eq!(builder.commands().len(), 1);

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    fn copy_duplicate_target() {
        let mut builder = CommandBuilder::new();
        let src1 = std::env::temp_dir().join("test_src1.txt");
        let src2 = std::env::temp_dir().join("test_src2.txt");
        let dst = std::env::temp_dir().join("test_dst_dup.txt");
        fs::write(&src1, "test1").unwrap();
        fs::write(&src2, "test2").unwrap();
        fs::write(&dst, "test").unwrap();

        builder.copy(&src1, &dst).unwrap();
        let result = builder.copy(&src2, &dst);

        assert!(result.is_err());
        match result.unwrap_err() {
            InstallerError::DuplicateCopyTarget { .. } => {},
            _ => panic!("Expected DuplicateCopyTarget error"),
        }

        fs::remove_file(&src1).ok();
        fs::remove_file(&src2).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    fn copy_mkdir_conflict() {
        let mut builder = CommandBuilder::new();
        let src = std::env::temp_dir().join("test_src.txt");
        let dst = std::env::temp_dir().join("test_mkdir_dst");
        fs::write(&src, "test").unwrap();
        fs::create_dir_all(&dst).unwrap();

        builder.mkdir(&dst).unwrap();
        assert!(builder.copy(&src, &dst).is_err());

        fs::remove_file(&src).ok();
        fs::remove_dir_all(&dst).ok();
    }

    #[test]
    fn mkdir() {
        let mut builder = CommandBuilder::new();
        let dir = std::env::temp_dir().join("test_mkdir");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        builder.mkdir(&dir).unwrap();
        assert_eq!(builder.commands().len(), 1);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mkdir_duplicate() {
        let mut builder = CommandBuilder::new();
        let dir = std::env::temp_dir().join("test_mkdir_dup");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        builder.mkdir(&dir).unwrap();
        builder.mkdir(&dir).unwrap();
        assert_eq!(builder.commands().len(), 1);

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mkdir_copy_conflict() {
        let mut builder = CommandBuilder::new();
        let src = std::env::temp_dir().join("test_src.txt");
        let dst = std::env::temp_dir().join("test_dst.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst, "test").unwrap();

        builder.copy(&src, &dst).unwrap();
        assert!(builder.mkdir(&dst).is_err());

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    fn making_directory() {
        let mut builder = CommandBuilder::new();
        let dir = std::env::temp_dir().join("test_mkdir_check");
        fs::create_dir_all(&dir).unwrap();

        builder.mkdir(&dir).unwrap();
        assert!(builder.making_directory(&dir));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn command_execute_copy() {
        let src = std::env::temp_dir().join("test_cmd_src.txt");
        let dst = std::env::temp_dir().join("test_cmd_dst.txt");
        fs::write(&src, "test").unwrap();

        let cmd = Command::Copy(CopyCommand::new(src.clone(), dst.clone()));
        let mut cleanup = Vec::new();
        cmd.execute_with_cleanup(&mut cleanup).unwrap();

        assert!(dst.exists());

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    fn command_execute_mkdir() {
        let dir = std::env::temp_dir().join("test_cmd_mkdir");
        let _ = fs::remove_dir(&dir);

        let cmd = Command::Mkdir(MakeDirectoryCommand::new(dir.clone()));
        let mut cleanup = Vec::new();
        cmd.execute_with_cleanup(&mut cleanup).unwrap();

        assert!(dir.exists());

        fs::remove_dir(&dir).ok();
    }

    #[test]
    fn command_execute_remove() {
        let file = std::env::temp_dir().join("test_cmd_rm.txt");
        fs::write(&file, "test").unwrap();

        let cmd = Command::Remove(RemoveCommand::new(file.clone()));
        let mut cleanup = Vec::new();
        cmd.execute_with_cleanup(&mut cleanup).unwrap();

        assert!(!file.exists());
    }

    #[test]
    #[cfg(unix)]
    fn command_execute_chmod() {
        let file = std::env::temp_dir().join("test_cmd_chmod.txt");
        fs::write(&file, "test").unwrap();

        let cmd =
            Command::Chmod(ChangeModeCommand::new(file.clone(), "0644".to_string(), false, false));
        let mut cleanup = Vec::new();
        cmd.execute_with_cleanup(&mut cleanup).unwrap();

        fs::remove_file(&file).ok();
    }

    #[test]
    #[cfg(unix)]
    fn command_execute_chown() {
        let file = std::env::temp_dir().join("test_cmd_chown.txt");
        fs::write(&file, "test").unwrap();

        let cmd = Command::Chown(ChangeOwnerCommand::new(file.clone(), None, None, false));
        let mut cleanup = Vec::new();
        cmd.execute_with_cleanup(&mut cleanup).unwrap();

        fs::remove_file(&file).ok();
    }

    #[test]
    fn copying_file() {
        let mut builder = CommandBuilder::new();
        let src = std::env::temp_dir().join("test_copying_src.txt");
        let dst = std::env::temp_dir().join("test_copying_dst.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst, "test").unwrap();

        assert!(!builder.copying_file(&dst));
        builder.copy(&src, &dst).unwrap();
        assert!(builder.copying_file(&dst));

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    #[cfg(unix)]
    fn command_execute_acl() {
        use crate::application_specification::Acl;

        let file = std::env::temp_dir().join("test_cmd_acl.txt");
        fs::write(&file, "test").unwrap();

        let acl = Acl::parse(&[]).unwrap();
        let cmd = Command::Acl(ChangeAclCommand::new(file.clone(), acl, false));
        let mut cleanup = Vec::new();
        let _ = cmd.execute_with_cleanup(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[test]
    #[cfg(unix)]
    fn command_execute_context() {
        use crate::application_specification::SeLinuxContext;

        let file = std::env::temp_dir().join("test_cmd_ctx.txt");
        fs::write(&file, "test").unwrap();

        let ctx = SeLinuxContext::new(None, "httpd_sys_content_t".to_string(), None);
        let cmd = Command::Context(ChangeContextCommand::new(file.clone(), ctx, false, false));
        let mut cleanup = Vec::new();
        let _ = cmd.execute_with_cleanup(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[cfg(unix)]
    #[test]
    fn find_matches_files() {
        use crate::application_specification::{ObjectType, Permission};

        let mut builder = CommandBuilder::new();
        let temp_dir = std::env::temp_dir();
        let src = temp_dir.join("test_find_src.txt");
        let dst1 = temp_dir.join("test_find_dst1.txt");
        let dst2 = temp_dir.join("test_find_dst2.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst1, "test").unwrap();
        fs::write(&dst2, "test").unwrap();

        builder.copy(&src, &dst1).unwrap();
        builder.copy(&src, &dst2).unwrap();

        let perm = Permission::new_for_test(
            temp_dir.to_string_lossy().to_string(),
            vec![ObjectType::File],
            vec![],
            None,
            None,
            None,
            None,
            None,
        );

        let matches = builder.find_matches(&perm).unwrap();
        assert_eq!(matches.len(), 2);

        fs::remove_file(&src).ok();
        fs::remove_file(&dst1).ok();
        fs::remove_file(&dst2).ok();
    }

    #[cfg(unix)]
    #[test]
    fn find_matches_directories() {
        use crate::application_specification::{ObjectType, Permission};

        let mut builder = CommandBuilder::new();
        let temp_dir = std::env::temp_dir();
        let dir1 = temp_dir.join("test_find_dir1");
        let dir2 = temp_dir.join("test_find_dir2");
        fs::create_dir_all(&dir1).unwrap();
        fs::create_dir_all(&dir2).unwrap();

        builder.mkdir(&dir1).unwrap();
        builder.mkdir(&dir2).unwrap();

        let perm = Permission::new_for_test(
            temp_dir.to_string_lossy().to_string(),
            vec![ObjectType::Directory],
            vec![],
            None,
            None,
            None,
            None,
            None,
        );

        let matches = builder.find_matches(&perm).unwrap();
        assert_eq!(matches.len(), 2);

        fs::remove_dir_all(&dir1).ok();
        fs::remove_dir_all(&dir2).ok();
    }

    #[cfg(unix)]
    #[test]
    fn find_matches_with_except() {
        // `except` is only valid on directory-type permissions
        // (`validate_file_permission` rejects it on file-type). Test the
        // legitimate shape here — directory targets with `except` filtering out
        // subdirectories.
        use crate::application_specification::{ObjectType, Permission};

        let mut builder = CommandBuilder::new();
        let temp_dir = std::env::temp_dir();
        let dir1 = temp_dir.join("test_except_dir1");
        let dir2 = temp_dir.join("test_except_exclude");
        fs::create_dir_all(&dir1).unwrap();
        fs::create_dir_all(&dir2).unwrap();

        builder.mkdir(&dir1).unwrap();
        builder.mkdir(&dir2).unwrap();

        let perm = Permission::new_for_test(
            temp_dir.to_string_lossy().to_string(),
            vec![ObjectType::Directory],
            vec!["*exclude*".to_string()],
            None,
            None,
            None,
            None,
            None,
        );

        let matches = builder.find_matches(&perm).unwrap();
        assert_eq!(matches.len(), 1);
        assert!(matches[0].to_string_lossy().contains("dir1"));

        fs::remove_dir_all(&dir1).ok();
        fs::remove_dir_all(&dir2).ok();
    }

    #[cfg(unix)]
    #[test]
    fn find_matches_file_type_with_except_does_not_reject() {
        // A permission whose `object:` is a DIRECTORY with `type: [file]` + a
        // non-`**` `pattern:` + `except:` selects files *under* that directory.
        // `find_matches` validates only the ACL per match, never the
        // pattern/except, so it must return the matched files here, not error.
        use crate::application_specification::{ObjectType, Permission};

        let mut builder = CommandBuilder::new();
        let temp_dir = std::env::temp_dir();
        let src = temp_dir.join("test_apply_reject_src.txt");
        let dst = temp_dir.join("test_apply_reject_dst.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst, "test").unwrap();

        builder.copy(&src, &dst).unwrap();

        let perm = Permission::new_for_test(
            temp_dir.to_string_lossy().to_string(),
            vec![ObjectType::File],
            vec!["*exclude*".to_string()],
            None,
            None,
            None,
            None,
            None,
        );

        let result = builder.find_matches(&perm);
        assert!(
            result.is_ok(),
            "find_matches validates only the ACL, not pattern/except: {result:?}",
        );
        // The one copied file matches (its name doesn't hit `*exclude*`).
        assert_eq!(result.unwrap().len(), 1);

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    #[cfg(unix)]
    fn set_permissions_with_mode() {
        use crate::application_specification::{Mode, ObjectType, Permission};

        let mut builder = CommandBuilder::new();
        let src = std::env::temp_dir().join("test_setperm_src.txt");
        let dst = std::env::temp_dir().join("test_setperm_dst.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst, "test").unwrap();

        builder.copy(&src, &dst).unwrap();

        let mode = Mode::from_octal("0644").unwrap();
        let perm = Permission::new_for_test(
            "**".to_string(),
            vec![ObjectType::File],
            vec![],
            None,
            None,
            Some(mode),
            None,
            None,
        );

        builder.set_permissions(&dst, &perm).unwrap();
        // Should have 1 copy + 1 chmod command
        assert_eq!(builder.commands().len(), 2);

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    #[cfg(unix)]
    fn set_permissions_with_acl() {
        use crate::application_specification::{Acl, ObjectType, Permission};

        let mut builder = CommandBuilder::new();
        let src = std::env::temp_dir().join("test_setperm_acl_src.txt");
        let dst = std::env::temp_dir().join("test_setperm_acl_dst.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst, "test").unwrap();

        builder.copy(&src, &dst).unwrap();

        let acl = Acl::parse(&["user:testuser:rwx".to_string()]).unwrap();
        let perm = Permission::new_for_test(
            "**".to_string(),
            vec![ObjectType::File],
            vec![],
            None,
            None,
            None,
            Some(acl),
            None,
        );

        builder.set_permissions(&dst, &perm).unwrap();
        // Should have 1 copy + 1 acl command
        assert_eq!(builder.commands().len(), 2);

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    #[cfg(unix)]
    fn set_permissions_with_context() {
        use crate::application_specification::{ObjectType, Permission, SeLinuxContext};

        let mut builder = CommandBuilder::new();
        let src = std::env::temp_dir().join("test_setperm_ctx_src.txt");
        let dst = std::env::temp_dir().join("test_setperm_ctx_dst.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst, "test").unwrap();

        builder.copy(&src, &dst).unwrap();

        let ctx = SeLinuxContext::new(None, "httpd_sys_content_t".to_string(), None);
        let perm = Permission::new_for_test(
            "**".to_string(),
            vec![ObjectType::File],
            vec![],
            None,
            None,
            None,
            None,
            Some(ctx),
        );

        builder.set_permissions(&dst, &perm).unwrap();
        // Should have 1 copy + 1 context command
        assert_eq!(builder.commands().len(), 2);

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    #[cfg(unix)]
    fn set_permissions_with_owner() {
        use crate::application_specification::{ObjectType, Permission};

        let mut builder = CommandBuilder::new();
        let src = std::env::temp_dir().join("test_setperm_own_src.txt");
        let dst = std::env::temp_dir().join("test_setperm_own_dst.txt");
        fs::write(&src, "test").unwrap();
        fs::write(&dst, "test").unwrap();

        builder.copy(&src, &dst).unwrap();

        let perm = Permission::new_for_test(
            "**".to_string(),
            vec![ObjectType::File],
            vec![],
            Some("testuser".to_string()),
            Some("testgroup".to_string()),
            None,
            None,
            None,
        );

        builder.set_permissions(&dst, &perm).unwrap();
        // Should have 1 copy + 1 chown command
        assert_eq!(builder.commands().len(), 2);

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    #[cfg(unix)]
    fn set_permissions_duplicate_error() {
        use crate::application_specification::{Mode, ObjectType, Permission};

        let mut builder = CommandBuilder::new();
        let file = std::env::temp_dir().join("test_setperm_dup.txt");
        fs::write(&file, "test").unwrap();

        let mode = Mode::from_octal("0644").unwrap();
        let perm = Permission::new_for_test(
            "**".to_string(),
            vec![ObjectType::File],
            vec![],
            None,
            None,
            Some(mode),
            None,
            None,
        );

        builder.set_permissions(&file, &perm).unwrap();
        let result = builder.set_permissions(&file, &perm);

        assert!(result.is_err());
        match result.unwrap_err() {
            InstallerError::DuplicatePermission { .. } => {},
            _ => panic!("Expected DuplicatePermission error"),
        }

        fs::remove_file(&file).ok();
    }

    #[test]
    fn command_to_h_remove() {
        let file = std::env::temp_dir().join("test_remove.txt");
        let cmd = Command::Remove(RemoveCommand::new(file.clone()));
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "remove");
    }

    #[test]
    #[cfg(unix)]
    fn command_to_h_acl() {
        use crate::application_specification::Acl;
        let file = std::env::temp_dir().join("test_acl.txt");
        fs::write(&file, "test").unwrap();

        let acl = Acl::parse(&["user:alice:rwx".to_string()]).unwrap();
        let cmd = Command::Acl(ChangeAclCommand::new(file.clone(), acl, false));
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "setfacl");
        fs::remove_file(&file).ok();
    }

    #[test]
    #[cfg(unix)]
    fn command_to_h_context() {
        use crate::application_specification::SeLinuxContext;
        let file = std::env::temp_dir().join("test_ctx.txt");
        fs::write(&file, "test").unwrap();

        let ctx = SeLinuxContext::new(None, "httpd_sys_content_t".to_string(), None);
        let cmd = Command::Context(ChangeContextCommand::new(file.clone(), ctx, false, false));
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "semanage");
        fs::remove_file(&file).ok();
    }
}
