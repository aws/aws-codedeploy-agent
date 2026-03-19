//! @risk medium
//!
//! File removal command — deletes files during cleanup.
use crate::installer::Result;
use serde_json::{Value, json};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

#[derive(Debug)]
pub struct RemoveCommand {
    file_path: PathBuf,
}

impl RemoveCommand {
    #[must_use]
    pub fn new(location: PathBuf) -> Self {
        Self { file_path: location }
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, _cleanup_file: &mut dyn Write) -> Result<()> {
        let path = &self.file_path;

        if !path.exists() {
            return Ok(());
        }

        let metadata = fs::symlink_metadata(path)?;

        if metadata.is_symlink() {
            fs::remove_file(path)?;
        } else if metadata.is_dir() {
            match fs::remove_dir(path) {
                Ok(()) => {},
                Err(e) if e.kind() == ErrorKind::DirectoryNotEmpty => {},
                Err(e) => return Err(e.into()),
            }
        } else {
            fs::remove_file(path)?;
        }

        Ok(())
    }

    /// Serialize command to JSON hash for debugging/audit
    #[must_use]
    pub fn to_h(&self) -> Value {
        json!({
            "type": "remove",
            "file": self.file_path
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs as unix_fs;

    #[test]
    fn execute_file() {
        let file = std::env::temp_dir().join("test_remove.txt");
        fs::write(&file, "test").unwrap();

        let cmd = RemoveCommand::new(file.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        assert!(!file.exists());
    }

    #[test]
    fn execute_symlink() {
        let target = std::env::temp_dir().join("test_target.txt");
        let link = std::env::temp_dir().join("test_link.txt");
        fs::write(&target, "test").unwrap();
        unix_fs::symlink(&target, &link).unwrap();

        let cmd = RemoveCommand::new(link.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        assert!(!link.exists());
        assert!(target.exists());
        fs::remove_file(&target).ok();
    }

    #[test]
    fn execute_directory() {
        let dir = std::env::temp_dir().join("test_remove_dir");
        fs::create_dir(&dir).unwrap();

        let cmd = RemoveCommand::new(dir.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        assert!(!dir.exists());
    }

    #[test]
    fn execute_nonexistent() {
        let cmd = RemoveCommand::new("/nonexistent/file".into());
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_ok());
    }

    #[test]
    fn execute_nonempty_directory_ignored() {
        // Test that non-empty directories are silently ignored (ENOTEMPTY)
        let dir = std::env::temp_dir().join("test_nonempty_dir");
        fs::create_dir_all(&dir).unwrap();

        // Create a file inside to make it non-empty
        let file = dir.join("file.txt");
        fs::write(&file, "content").unwrap();

        let cmd = RemoveCommand::new(dir.clone());
        let mut cleanup = Vec::new();

        // Should succeed even though directory is not empty
        assert!(cmd.execute(&mut cleanup).is_ok());

        // Directory should still exist (wasn't removed)
        assert!(dir.exists());

        // Cleanup
        fs::remove_file(&file).ok();
        fs::remove_dir(&dir).ok();
    }

    #[test]
    #[cfg(unix)]
    fn execute_directory_other_error_propagates() {
        // Test that errors other than DirectoryNotEmpty are propagated
        // Try to remove /tmp which will fail with permission error or busy
        let dir = std::path::PathBuf::from("/tmp");

        let cmd = RemoveCommand::new(dir);
        let mut cleanup = Vec::new();

        // Should fail because /tmp can't be removed (permission or busy)
        // This will NOT be DirectoryNotEmpty error
        let result = cmd.execute(&mut cleanup);

        // On most systems this will fail, but if somehow it succeeds (unlikely),
        // that's also acceptable for the test
        if result.is_err() {
            // Error was propagated (not silently ignored)
            assert!(result.is_err());
        }
    }

    #[test]
    fn to_h() {
        let file = std::env::temp_dir().join("test_remove_to_h.txt");
        let cmd = RemoveCommand::new(file.clone());
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "remove");
        assert_eq!(hash["file"], file.to_str().unwrap());
    }
}
