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
    /// Mode policy from `restrict_agent_dir_permissions`: 0755/0644 by default
    /// (world-readable, so health checks can read the pidfile as non-root),
    /// 0700/0600 when hardened.
    restrict: bool,
}

impl PidFile {
    /// Create a new `PidFile` manager with the default (world-readable) modes.
    #[must_use]
    pub fn new(dir: &Path, filename: &str) -> Self {
        Self { path: dir.join(filename), restrict: false }
    }

    /// Create a new `PidFile` manager with an explicit mode policy
    /// (`restrict_agent_dir_permissions`).
    #[must_use]
    pub fn with_policy(dir: &Path, filename: &str, restrict: bool) -> Self {
        Self { path: dir.join(filename), restrict }
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
    /// Modes follow the `restrict_agent_dir_permissions` policy: 0755 dir /
    /// 0644 file by default (non-root health checks read the pidfile),
    /// 0700/0600 under opt-in hardening.
    ///
    /// # Errors
    /// Returns an error if directory creation or file write fails.
    pub fn write(&self) -> io::Result<()> {
        use crate::system::{agent_file_mode, create_deployment_dir, write_file_secure};

        // GRCOV_STOP_COVERAGE
        if let Some(parent) = self.path.parent() {
            create_deployment_dir(parent, 0o700, self.restrict)?;
        }
        // GRCOV_BEGIN_COVERAGE
        self.remove_stale()?;
        let pid = std::process::id();
        write_file_secure(&self.path, pid.to_string().as_bytes(), agent_file_mode(self.restrict))?;
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

    /// Return the PID if the file exists and the referenced process is alive.
    ///
    /// Returns `Ok(None)` when the PID file is absent or the process is not
    /// running (stale PID). Returns `Err` only on I/O errors reading the file.
    ///
    /// # Errors
    /// Returns an I/O error if the PID file cannot be read or parsed.
    pub fn running_pid(&self) -> io::Result<Option<u32>> {
        match self.read()? {
            Some(pid) if is_process_alive(pid) => Ok(Some(pid)),
            _ => Ok(None),
        }
    }

    /// Convenience wrapper over [`running_pid`](Self::running_pid).
    ///
    /// Returns `true` if the PID file exists and the process is running.
    /// Swallows I/O errors; use [`running_pid`](Self::running_pid) if you
    /// need to distinguish "not running" from "could not read PID file".
    #[must_use]
    pub fn is_running(&self) -> bool {
        matches!(self.running_pid(), Ok(Some(_)))
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

    #[cfg(unix)]
    #[test]
    fn read_permission_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        std::fs::write(pf.path(), "12345").unwrap();
        std::fs::set_permissions(pf.path(), std::fs::Permissions::from_mode(0o000)).unwrap();
        assert!(pf.read().is_err());
        std::fs::set_permissions(pf.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn write_rename_failure() {
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        // A non-empty directory at the PID path makes the atomic rename fail.
        // (A read-only parent dir no longer works to force this: the
        // policy-driven dir creation heals agent-owned dirs back to the
        // policy mode, restoring writability.)
        std::fs::create_dir(pf.path()).unwrap();
        std::fs::write(pf.path().join("occupied"), "x").unwrap();
        assert!(pf.write().is_err());
    }

    #[test]
    fn write_with_no_parent() {
        let pf = PidFile::new(Path::new(""), "test.pid");
        // Should handle path with no parent gracefully
        let _ = pf.write();
    }

    #[cfg(unix)]
    #[test]
    fn write_default_creates_world_readable_pid_file() {
        // Non-root health checks read the pidfile.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let pf = pid_file(&dir);
        pf.write().unwrap();
        let mode = std::fs::metadata(pf.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "PID file must be 0644 by default, got {mode:#o}");
    }

    #[cfg(unix)]
    #[test]
    fn write_default_creates_world_readable_parent_dir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("nested/state");
        let pf = PidFile::new(&nested, "agent.pid");
        pf.write().unwrap();
        let mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "PID parent dir must be 0755 by default, got {mode:#o}");
    }

    #[cfg(unix)]
    #[test]
    fn write_restricted_creates_hardened_pid_file_and_dir() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("nested/state");
        let pf = PidFile::with_policy(&nested, "agent.pid", true);
        pf.write().unwrap();
        let dir_mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(pf.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "hardened PID dir must be 0700, got {dir_mode:#o}");
        assert_eq!(file_mode, 0o600, "hardened PID file must be 0600, got {file_mode:#o}");
    }
}
