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
    /// Returns an error if the directory cannot be created, unless a directory
    /// already exists at the path.
    pub fn execute(&self, cleanup_file: &mut dyn Write) -> Result<()> {
        match fs::create_dir(&self.directory) {
            Ok(()) => {
                writeln!(cleanup_file, "{}", self.directory.display())?;
            },
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && self.directory.is_dir() => {
            },
            Err(e) => return Err(e.into()),
        }

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
        cmd.execute(&mut cleanup).unwrap();

        assert!(dir.is_dir());
        assert!(cleanup.is_empty());

        fs::remove_dir(&dir).ok();
    }

    #[test]
    fn execute_file_exists() {
        let path = std::env::temp_dir().join("test_mkdir_file_exists");
        let _ = fs::remove_dir_all(&path);
        fs::write(&path, b"").unwrap();

        let cmd = MakeDirectoryCommand::new(path.clone());
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());
        assert!(cleanup.is_empty());

        fs::remove_file(&path).ok();
    }

    #[cfg(unix)]
    #[test]
    fn execute_symlink_to_dir_exists() {
        let target = std::env::temp_dir().join("test_mkdir_symlink_target");
        let link = std::env::temp_dir().join("test_mkdir_symlink_link");
        let _ = fs::remove_file(&link);
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let cmd = MakeDirectoryCommand::new(link.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();
        assert!(cleanup.is_empty());

        fs::remove_file(&link).ok();
        fs::remove_dir(&target).ok();
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
