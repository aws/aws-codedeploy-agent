//! `chown` command — sets file ownership.
use crate::installer::Result;
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ChangeOwnerCommand {
    object: PathBuf,
    owner: Option<String>,
    group: Option<String>,
    reject_symlink_target: bool,
}

impl ChangeOwnerCommand {
    #[must_use]
    pub fn new(
        object: PathBuf,
        owner: Option<String>,
        group: Option<String>,
        reject_symlink_target: bool,
    ) -> Self {
        Self { object, owner, group, reject_symlink_target }
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, _cleanup_file: &mut dyn Write) -> Result<()> {
        use nix::fcntl::AtFlags;
        use nix::unistd::{Group, User, fchownat};

        // SECURITY: under `reject_symlink_permission_targets`, reject a symlinked
        // destination, then chown no-follow, so a raced-in symlink can't redirect
        // the chown.
        if self.reject_symlink_target {
            crate::installer::safe_fs::reject_symlink_dest(&self.object)?;
        }

        let uid = self
            .owner
            .as_ref()
            .and_then(|o| User::from_name(o).ok().flatten())
            .map(|u| u.uid);

        let gid = self
            .group
            .as_ref()
            .and_then(|g| Group::from_name(g).ok().flatten())
            .map(|g| g.gid);

        // By default the chown follows symlinks (backwards-compatible behavior),
        // which `fchownat` with empty flags gives. Under the opt-in flag we pass
        // `AT_SYMLINK_NOFOLLOW` (= lchown) so the chown lands on the link itself,
        // never its target.
        let flags = if self.reject_symlink_target {
            AtFlags::AT_SYMLINK_NOFOLLOW
        } else {
            AtFlags::empty()
        };
        fchownat(None, &self.object, uid, gid, flags)
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;

        Ok(())
    }

    /// Serialize command to JSON hash for debugging/audit
    #[must_use]
    pub fn to_h(&self) -> Value {
        json!({
            "type": "chown",
            "owner": self.owner,
            "group": self.group,
            "file": self.object
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn execute_nonexistent_file() {
        let cmd = ChangeOwnerCommand::new(
            "/nonexistent/file".into(),
            Some("root".to_string()),
            None,
            false,
        );
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());
    }

    #[test]
    fn execute_invalid_user() {
        let file = std::env::temp_dir().join("test_owner.txt");
        fs::write(&file, "test").unwrap();

        let cmd = ChangeOwnerCommand::new(
            file.clone(),
            Some("nonexistentuser999".to_string()),
            None,
            false,
        );
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
        assert!(result.is_ok());
    }

    #[test]
    fn execute_invalid_group() {
        let file = std::env::temp_dir().join("test_group.txt");
        fs::write(&file, "test").unwrap();

        let cmd = ChangeOwnerCommand::new(
            file.clone(),
            None,
            Some("nonexistentgroup999".to_string()),
            false,
        );
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
        assert!(result.is_ok());
    }

    #[test]
    fn execute_both_none() {
        let file = std::env::temp_dir().join("test_none.txt");
        fs::write(&file, "test").unwrap();

        let cmd = ChangeOwnerCommand::new(file.clone(), None, None, false);
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_ok());

        fs::remove_file(&file).ok();
    }

    #[test]
    fn to_h() {
        let cmd = ChangeOwnerCommand::new(
            "/path/to/file.txt".into(),
            Some("user".to_string()),
            Some("group".to_string()),
            false,
        );
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "chown");
        assert_eq!(hash["owner"], "user");
        assert_eq!(hash["group"], "group");
        assert_eq!(hash["file"], "/path/to/file.txt");
    }

    #[test]
    fn to_h_with_none() {
        let cmd = ChangeOwnerCommand::new(
            "/path/to/file.txt".into(),
            None,
            Some("group".to_string()),
            false,
        );
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "chown");
        assert!(hash["owner"].is_null());
        assert_eq!(hash["group"], "group");
        assert_eq!(hash["file"], "/path/to/file.txt");
    }
}
