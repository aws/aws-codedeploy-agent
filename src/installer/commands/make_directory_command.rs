//! `mkdir` command — creates destination directories.
use crate::installer::Result;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub struct MakeDirectoryCommand {
    directory: PathBuf,
}

impl MakeDirectoryCommand {
    #[must_use]
    pub fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, cleanup_file: &mut dyn Write) -> Result<()> {
        fs::create_dir(&self.directory)?;
        writeln!(cleanup_file, "{}", self.directory.display())?;
        Ok(())
    }

    /// Serialize command to JSON hash for debugging/audit
    #[must_use]
    pub fn to_h(&self) -> Value {
        json!({
            "type": "mkdir",
            "directory": self.directory
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn execute_success() {
        let dir = std::env::temp_dir().join("test_mkdir");
        let _ = fs::remove_dir(&dir);

        let cmd = MakeDirectoryCommand::new(dir.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        assert!(dir.exists());
        assert!(dir.is_dir());
        assert!(String::from_utf8_lossy(&cleanup).contains(&dir.display().to_string()));

        fs::remove_dir(&dir).ok();
    }

    #[test]
    fn execute_already_exists() {
        let dir = std::env::temp_dir().join("test_mkdir_exists");
        fs::create_dir_all(&dir).unwrap();

        let cmd = MakeDirectoryCommand::new(dir.clone());
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());

        fs::remove_dir(&dir).ok();
    }

    #[test]
    fn execute_parent_not_exists() {
        let cmd = MakeDirectoryCommand::new("/nonexistent/parent/dir".into());
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());
    }

    #[test]
    fn to_h() {
        let cmd = MakeDirectoryCommand::new("/path/to/directory".into());
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "mkdir");
        assert_eq!(hash["directory"], "/path/to/directory");
    }
}
