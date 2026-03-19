//! @risk medium
//!
//! PID file management for the agent daemon.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use super::is_process_alive;

/// Manages a PID file at `{dir}/{filename}`.
#[derive(Debug)]
pub struct PidFile {
    path: PathBuf,
}

impl PidFile {
    /// Create a new `PidFile` manager for the given directory and filename.
    #[must_use]
    pub fn new(dir: &Path, filename: &str) -> Self {
        Self { path: dir.join(filename) }
    }

    /// Path to the PID file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Write the current process's PID to the file.
    ///
    /// Creates parent directories if needed. Removes stale PID files first.
    /// Uses atomic temp file + rename for crash safety.
    ///
    /// # Errors
    /// Returns an error if directory creation or file write fails.
    pub fn write(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.remove_stale()?;
        let pid = std::process::id();
        // Append `.tmp.{pid}` to the full path rather than using `with_extension`
        // which replaces the last extension. This is more predictable if the
        // PID filename ever contains multiple dots.
        let tmp = self.path.with_file_name(format!(
            "{}.tmp.{pid}",
            self.path.file_name().unwrap_or_default().to_string_lossy()
        ));
        fs::write(&tmp, pid.to_string())?;
        if let Err(e) = fs::rename(&tmp, &self.path) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        debug!("Wrote PID {pid} to {}", self.path.display());
        Ok(())
    }

    /// Read the PID from the file. Returns `None` if the file doesn't exist.
    ///
    /// # Errors
    /// Returns an error if the file exists but can't be read or parsed.
    pub fn read(&self) -> io::Result<Option<u32>> {
        match fs::read_to_string(&self.path) {
            Ok(contents) => {
                let pid = contents
                    .trim()
                    .parse::<u32>()
                    .map_err(|e| io::Error::other(format!("invalid PID: {e}")))?;
                Ok(Some(pid))
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Remove the PID file.
    ///
    /// # Errors
    /// Returns an error if the file exists but can't be removed.
    pub fn remove(&self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => {
                debug!("Removed PID file {}", self.path.display());
                Ok(())
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// Check if the process recorded in the PID file is still alive.
    /// If not, remove the stale file.
    fn remove_stale(&self) -> io::Result<()> {
        if let Some(pid) = self.read()?
            && !is_process_alive(pid)
        {
            warn!("Removing stale PID file (pid {pid} not running)");
            self.remove()?;
        }
        Ok(())
    }

    /// Check if a process with the given PID is alive.
    /// Returns `true` if the PID file exists and the process is running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        match self.read() {
            Ok(Some(pid)) => is_process_alive(pid),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn pid_file(dir: &TempDir) -> PidFile {
        PidFile::new(dir.path(), "test.pid")
    }

    #[test]
    fn write_and_read_pid() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        pf.write().unwrap();
        let pid = pf.read().unwrap();
        assert_eq!(pid, Some(std::process::id()));
    }

    #[test]
    fn read_nonexistent_returns_none() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        assert_eq!(pf.read().unwrap(), None);
    }

    #[test]
    fn read_invalid_content_returns_error() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        std::fs::write(pf.path(), "not-a-number").unwrap();
        assert!(pf.read().is_err());
    }

    #[test]
    fn remove_nonexistent_is_ok() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        assert!(pf.remove().is_ok());
    }

    #[test]
    fn remove_existing_file() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        pf.write().unwrap();
        assert!(pf.path().exists());
        pf.remove().unwrap();
        assert!(!pf.path().exists());
    }

    #[test]
    fn is_running_true_for_current_process() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        pf.write().unwrap();
        assert!(pf.is_running());
    }

    #[test]
    fn is_running_false_when_no_file() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        assert!(!pf.is_running());
    }

    #[test]
    fn is_running_false_for_dead_pid() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        std::fs::write(pf.path(), "99999999").unwrap();
        assert!(!pf.is_running());
    }

    #[test]
    fn write_creates_parent_dirs() {
        let dir = TempDir::new().unwrap();
        let pf = PidFile::new(&dir.path().join("nested").join("dir"), "test.pid");
        pf.write().unwrap();
        assert!(pf.path().exists());
    }

    #[test]
    fn write_removes_stale_pid_file() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        std::fs::write(pf.path(), "99999999").unwrap();
        pf.write().unwrap();
        assert_eq!(pf.read().unwrap(), Some(std::process::id()));
    }

    #[test]
    fn path_returns_correct_path() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        assert_eq!(pf.path(), dir.path().join("test.pid"));
    }

    #[test]
    fn read_permission_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        std::fs::write(pf.path(), "12345").unwrap();
        std::fs::set_permissions(pf.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(pf.read().is_err());
        std::fs::set_permissions(pf.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    #[test]
    fn remove_permission_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        pf.write().unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(pf.remove().is_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn write_rename_failure() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        // Create the target file as read-only to make rename fail
        std::fs::write(pf.path(), "12345").unwrap();
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o444)).unwrap();
        assert!(pf.write().is_err());
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn write_with_no_parent() {
        let pf = PidFile::new(Path::new(""), "test.pid");
        // Should handle path with no parent gracefully
        let _ = pf.write();
    }
}
