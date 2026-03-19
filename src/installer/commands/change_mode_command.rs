//! @risk medium
//!
//! `chmod` command — sets file permissions.
use crate::installer::Result;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Debug)]
pub struct ChangeModeCommand {
    object: PathBuf,
    mode: String,
}

impl ChangeModeCommand {
    #[must_use]
    pub fn new(object: PathBuf, mode: String) -> Self {
        Self { object, mode }
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, _cleanup_file: &mut dyn Write) -> Result<()> {
        let mode = u32::from_str_radix(&self.mode, 8)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "Invalid mode"))?;

        let perms = fs::Permissions::from_mode(mode);
        fs::set_permissions(&self.object, perms)?;
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

        let cmd = ChangeModeCommand::new(file.clone(), "0644".to_string());
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

        let cmd = ChangeModeCommand::new(file.clone(), "invalid".to_string());
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_nonexistent_file() {
        let cmd = ChangeModeCommand::new("/nonexistent/file".into(), "0644".to_string());
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());
    }

    #[test]
    fn to_h() {
        let cmd = ChangeModeCommand::new("/path/to/file.txt".into(), "0755".to_string());
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "chmod");
        assert_eq!(hash["mode"], "0755");
        assert_eq!(hash["file"], "/path/to/file.txt");
    }
}
