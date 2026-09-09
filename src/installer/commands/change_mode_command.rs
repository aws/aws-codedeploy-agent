//! `chmod` command — sets file permissions.
use crate::installer::{InstallerError, Result};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ChangeModeCommand {
    object: PathBuf,
    mode: String,
    reject_unsafe: bool,
    reject_symlink_target: bool,
}

impl ChangeModeCommand {
    #[must_use]
    pub fn new(
        object: PathBuf,
        mode: String,
        reject_unsafe: bool,
        reject_symlink_target: bool,
    ) -> Self {
        Self { object, mode, reject_unsafe, reject_symlink_target }
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, _cleanup_file: &mut dyn Write) -> Result<()> {
        use nix::sys::stat::{Mode, fchmod};
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::OpenOptionsExt;

        let mode = u32::from_str_radix(&self.mode, 8)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid mode"))?;

        if self.reject_unsafe && mode & 0o6000 != 0 {
            return Err(InstallerError::UnsafePermissionRejected {
                object: self.object.clone(),
                mode: format!("{mode:04o}"),
            });
        }

        let mode_bits = Mode::from_bits_truncate(mode);

        // By default the chmod follows symlinks (backwards-compatible behavior),
        // which a path-based `chmod` gives.
        if !self.reject_symlink_target {
            nix::sys::stat::fchmodat(
                None,
                &self.object,
                mode_bits,
                nix::sys::stat::FchmodatFlags::FollowSymlink,
            )
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
            return Ok(());
        }

        // SECURITY: under `reject_symlink_permission_targets`, reject a symlinked
        // destination, then chmod via an O_NOFOLLOW fd so a raced-in symlink
        // can't redirect the mode change. Linux `fchmodat` ignores
        // AT_SYMLINK_NOFOLLOW for regular files (ENOTSUP), so open O_NOFOLLOW and
        // `fchmod` the fd.
        crate::installer::safe_fs::reject_symlink_dest(&self.object)?;

        let file = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(nix::libc::O_NOFOLLOW)
            .open(&self.object)?;
        fchmod(file.as_raw_fd(), mode_bits)
            .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;
        Ok(())
    }

    /// Serialize command to JSON hash for debugging/audit
    #[must_use]
    pub fn to_h(&self) -> Value {
        json!({
            "type": "chmod",
            "mode": self.mode,
            "file": self.object
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn execute_valid_mode() {
        let file = std::env::temp_dir().join("test_mode.txt");
        fs::write(&file, "test").unwrap();

        let cmd = ChangeModeCommand::new(file.clone(), "0644".to_string(), false, false);
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        let metadata = fs::metadata(&file).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o644);

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_invalid_mode() {
        let file = std::env::temp_dir().join("test_invalid.txt");
        fs::write(&file, "test").unwrap();

        let cmd = ChangeModeCommand::new(file.clone(), "invalid".to_string(), false, false);
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_nonexistent_file() {
        let cmd =
            ChangeModeCommand::new("/nonexistent/file".into(), "0644".to_string(), false, false);
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());
    }

    #[test]
    fn to_h() {
        let cmd =
            ChangeModeCommand::new("/path/to/file.txt".into(), "0755".to_string(), false, false);
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "chmod");
        assert_eq!(hash["mode"], "0755");
        assert_eq!(hash["file"], "/path/to/file.txt");
    }

    #[test]
    fn rejects_suid_when_flag_enabled() {
        let file = std::env::temp_dir().join("test_mode_suid_reject.txt");
        fs::write(&file, "test").unwrap();

        for mode_str in ["4755", "2755", "6755"] {
            let cmd = ChangeModeCommand::new(file.clone(), mode_str.to_string(), true, false);
            let mut cleanup = Vec::new();
            let result = cmd.execute(&mut cleanup);

            assert!(result.is_err(), "mode {mode_str} should be rejected");
            match result.unwrap_err() {
                InstallerError::UnsafePermissionRejected { mode, .. } => {
                    assert!(
                        mode.contains(mode_str),
                        "error should carry mode {mode_str}, got {mode}"
                    );
                },
                other => panic!("expected UnsafePermissionRejected for {mode_str}, got {other:?}"),
            }
        }

        fs::remove_file(&file).ok();
    }

    #[test]
    fn allows_suid_when_flag_disabled() {
        let file = std::env::temp_dir().join("test_mode_suid_allow.txt");
        fs::write(&file, "test").unwrap();

        let cmd = ChangeModeCommand::new(file.clone(), "4755".to_string(), false, false);
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        let metadata = fs::metadata(&file).unwrap();
        assert_eq!(
            metadata.permissions().mode() & 0o7777,
            0o4755,
            "SUID bit must be preserved when flag is disabled"
        );

        fs::remove_file(&file).ok();
    }

    #[test]
    fn allows_safe_mode_when_flag_enabled() {
        let file = std::env::temp_dir().join("test_mode_safe_with_flag.txt");
        fs::write(&file, "test").unwrap();

        let cmd = ChangeModeCommand::new(file.clone(), "0755".to_string(), true, false);
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        let metadata = fs::metadata(&file).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o755);

        fs::remove_file(&file).ok();
    }
}
