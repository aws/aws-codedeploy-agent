//! @risk medium
//!
//! Deployment tracking functionality
//!
//! This module provides traits and implementations for tracking active deployments
//! using file-based persistence. The tracker maintains deployment state across
//! agent restarts and automatically cleans up stale deployment files.

use super::deployment_tracker::{ActiveDeployment, DeploymentTracker, DeploymentTrackerError};
use crate::system::{PlatformFileOperations, SystemFileOperations};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::warn;

#[derive(Debug)]
pub struct FileBasedDeploymentTracker<F: PlatformFileOperations = SystemFileOperations> {
    tracking_dir: PathBuf,
    stale_timeout: Duration,
    file_ops: F,
}

impl<F: PlatformFileOperations> FileBasedDeploymentTracker<F> {
    const DEFAULT_STALE_TIMEOUT_SECS: u64 = 86400; // 24 hours

    #[must_use]
    pub fn new(tracking_dir: PathBuf) -> Self
    where
        F: Default,
    {
        Self {
            tracking_dir,
            stale_timeout: Duration::from_secs(Self::DEFAULT_STALE_TIMEOUT_SECS),
            file_ops: F::default(),
        }
    }

    #[cfg(test)]
    pub fn new_with_ops(tracking_dir: PathBuf, file_ops: F) -> Self {
        Self {
            tracking_dir,
            stale_timeout: Duration::from_secs(Self::DEFAULT_STALE_TIMEOUT_SECS),
            file_ops,
        }
    }

    fn tracking_file_path(&self, deployment_id: &str) -> PathBuf {
        self.tracking_dir.join(deployment_id)
    }

    fn is_stale(&self, path: &PathBuf) -> Result<bool, DeploymentTrackerError> {
        let metadata = fs::metadata(path)?;
        let modified = metadata.modified()?;
        let elapsed = SystemTime::now().duration_since(modified).unwrap_or(Duration::ZERO);
        Ok(elapsed > self.stale_timeout)
    }

    fn delete_stale_files(&self) {
        if !self.tracking_dir.exists() {
            return;
        }

        let Ok(entries) = fs::read_dir(&self.tracking_dir) else {
            eprintln!(
                "Warning: could not read tracking directory for stale cleanup: {}",
                self.tracking_dir.display()
            );
            return; // Best effort: skip cleanup if dir can't be read
        };

        for entry in entries {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.is_file() && self.is_stale(&path).unwrap_or(false) {
                // Best effort deletion - log but don't fail if removal fails
                if let Err(e) = fs::remove_file(&path) {
                    eprintln!(
                        "Warning: Failed to delete stale tracking file {}: {e}",
                        path.display()
                    );
                }
            }
        }
    }
}

impl<F: PlatformFileOperations> DeploymentTracker for FileBasedDeploymentTracker<F> {
    fn start_tracking(
        &self,
        deployment_id: &str,
        host_command_identifier: &str,
    ) -> Result<(), DeploymentTrackerError> {
        fs::create_dir_all(&self.tracking_dir)?;
        let path = self.tracking_file_path(deployment_id);
        self.file_ops.write_with_retry(&path, host_command_identifier)?;
        Ok(())
    }

    fn stop_tracking(&self, deployment_id: &str) -> Result<(), DeploymentTrackerError> {
        let path = self.tracking_file_path(deployment_id);
        if path.exists() {
            fs::remove_file(path)?;
        } else {
            warn!("the tracking file does not exist");
        }
        Ok(())
    }

    fn get_active_deployment(&self) -> Result<Option<ActiveDeployment>, DeploymentTrackerError> {
        self.delete_stale_files();

        let entries = match fs::read_dir(&self.tracking_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let mut most_recent: Option<(PathBuf, SystemTime)> = None;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let metadata = fs::metadata(&path)?;
            let modified = metadata.modified()?;

            match &most_recent {
                Some((_, current_time)) if modified > *current_time => {
                    most_recent = Some((path, modified));
                },
                None => {
                    most_recent = Some((path, modified));
                },
                _ => {},
            }
        }

        if let Some((path, modified)) = most_recent {
            let deployment_id = path
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or_else(|| {
                    DeploymentTrackerError::InvalidDeploymentId("Invalid file name".to_string())
                })?
                .to_string();

            let host_command_identifier = fs::read_to_string(&path)?;
            let timestamp = modified.duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_secs();

            Ok(Some(ActiveDeployment { deployment_id, host_command_identifier, timestamp }))
        } else {
            Ok(None)
        }
    }

    fn is_deployment_in_progress(&self) -> Result<bool, DeploymentTrackerError> {
        Ok(self.get_active_deployment()?.is_some())
    }

    fn clean_all(&self) -> Result<(), DeploymentTrackerError> {
        if self.tracking_dir.exists() {
            fs::remove_dir_all(&self.tracking_dir)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::{MockFileOperations, SystemFileOperations};
    use tempfile::TempDir;

    #[test]
    fn new() {
        let dir = TempDir::new().unwrap();
        let _tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());
        // Can't test private field, just ensure it doesn't panic
    }

    #[test]
    fn start_tracking() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-123", "cmd-456").unwrap();

        let file_path = dir.path().join("d-123");
        assert!(file_path.exists());
        let content = std::fs::read_to_string(file_path).unwrap();
        assert_eq!(content, "cmd-456");
    }

    #[test]
    fn stop_tracking() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-123", "cmd-456").unwrap();
        tracker.stop_tracking("d-123").unwrap();

        let file_path = dir.path().join("d-123");
        assert!(!file_path.exists());
    }

    #[test]
    fn stop_tracking_nonexistent() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        let result = tracker.stop_tracking("d-nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn is_deployment_in_progress_empty() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        assert!(!tracker.is_deployment_in_progress().unwrap());
    }

    #[test]
    fn is_deployment_in_progress_with_active() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-123", "cmd-456").unwrap();
        assert!(tracker.is_deployment_in_progress().unwrap());
    }

    #[test]
    fn get_active_deployment() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-123", "cmd-456").unwrap();

        let active = tracker.get_active_deployment().unwrap();
        assert!(active.is_some());
        let deployment = active.unwrap();
        assert_eq!(deployment.deployment_id, "d-123");
        assert_eq!(deployment.host_command_identifier, "cmd-456");
    }

    #[test]
    fn get_active_deployment_none() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        let active = tracker.get_active_deployment().unwrap();
        assert!(active.is_none());
    }

    #[test]
    fn get_active_deployment_most_recent() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-1", "cmd-1").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        tracker.start_tracking("d-2", "cmd-2").unwrap();

        let active = tracker.get_active_deployment().unwrap().unwrap();
        assert_eq!(active.deployment_id, "d-2");
    }

    #[test]
    fn clean_all() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-1", "cmd-1").unwrap();
        tracker.start_tracking("d-2", "cmd-2").unwrap();

        tracker.clean_all().unwrap();
        assert!(!dir.path().exists());
    }

    #[test]
    fn clean_all_nonexistent_dir() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();
        drop(dir); // Remove directory

        let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(path);
        let result = tracker.clean_all();
        assert!(result.is_ok());
    }

    #[test]
    fn stop_tracking_nonexistent_file() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        // Should not error when file doesn't exist
        let result = tracker.stop_tracking("nonexistent");
        assert!(result.is_ok());
    }

    #[test]
    fn stale_file_deletion() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        // Create a tracking file
        tracker.start_tracking("d-old", "cmd-old").unwrap();
        let file_path = dir.path().join("d-old");

        // Manually set file modification time to 25 hours ago (past 24h threshold)
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
        filetime::set_file_mtime(&file_path, filetime::FileTime::from_system_time(old_time))
            .unwrap();

        // get_active_deployment should trigger stale file cleanup
        let result = tracker.get_active_deployment().unwrap();
        assert!(result.is_none());

        // File should be deleted
        assert!(!file_path.exists());
    }

    #[test]
    fn get_active_deployment_nonexistent_dir() {
        let dir = TempDir::new().unwrap();
        let nonexistent = dir.path().join("nonexistent");
        let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(nonexistent);

        // Should return None when directory doesn't exist
        let result = tracker.get_active_deployment().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn get_active_deployment_single_file() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        // Create only one tracking file
        tracker.start_tracking("d-single", "cmd-single").unwrap();

        let active = tracker.get_active_deployment().unwrap().unwrap();
        assert_eq!(active.deployment_id, "d-single");
        assert_eq!(active.host_command_identifier, "cmd-single");
    }

    #[test]
    fn write_with_retry_failure() {
        let dir = TempDir::new().unwrap();
        let mock_ops = MockFileOperations { should_fail: true };
        let tracker = FileBasedDeploymentTracker::<MockFileOperations>::new_with_ops(
            dir.path().to_path_buf(),
            mock_ops,
        );

        // Should fail when mock is configured to fail
        let result = tracker.start_tracking("d-fail", "cmd-fail");
        assert!(result.is_err());
    }

    #[test]
    fn write_with_retry_success() {
        let dir = TempDir::new().unwrap();
        let mock_ops = MockFileOperations { should_fail: false };
        let tracker = FileBasedDeploymentTracker::<MockFileOperations>::new_with_ops(
            dir.path().to_path_buf(),
            mock_ops,
        );

        // Should succeed when mock is configured to succeed
        let result = tracker.start_tracking("d-success", "cmd-success");
        assert!(result.is_ok());
    }
}

#[cfg(test)]
mod edge_case_tests {
    use super::*;
    use crate::system::SystemFileOperations;
    use tempfile::TempDir;

    #[test]
    fn stale_file_deletion_failure_warning() {
        // Test the eprintln warning path when stale file removal fails
        use std::os::unix::fs::PermissionsExt;
        struct RestorePerms<'a>(&'a std::path::Path);
        impl Drop for RestorePerms<'_> {
            fn drop(&mut self) {
                if let Ok(md) = std::fs::metadata(self.0) {
                    let mut p = md.permissions();
                    p.set_mode(0o755);
                    let _ = std::fs::set_permissions(self.0, p);
                }
            }
        }

        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-stale", "cmd-stale").unwrap();
        let file_path = dir.path().join("d-stale");

        // Set file to stale (25 hours ago)
        let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
        filetime::set_file_mtime(&file_path, filetime::FileTime::from_system_time(old_time))
            .unwrap();

        // Make file undeletable by removing write permission on parent dir
        // Use a scope guard to ensure permissions are always restored (even on panic)
        let _guard = RestorePerms(dir.path());

        let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
        perms.set_mode(0o555);
        std::fs::set_permissions(dir.path(), perms).unwrap();

        // get_active_deployment triggers stale cleanup — removal fails, prints warning
        let result = tracker.get_active_deployment();

        // Result should still be Ok (best-effort deletion)
        assert!(result.is_ok());
    }

    #[test]
    fn get_active_deployment_permission_error() {
        // Test the non-NotFound error path in get_active_deployment
        // delete_stale_files has a guard: if !self.tracking_dir.exists() { return Ok(()) }
        // So we need delete_stale_files to succeed, then read_dir to fail with non-NotFound.
        // Replace the directory with a file after delete_stale_files' exists() check passes.
        let dir = TempDir::new().unwrap();
        let tracker_dir = dir.path().join("tracker");
        std::fs::create_dir_all(&tracker_dir).unwrap();

        let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(tracker_dir.clone());

        // Remove dir and replace with a file — read_dir on a file gives NotADirectory
        std::fs::remove_dir(&tracker_dir).unwrap();
        std::fs::write(&tracker_dir, "not a directory").unwrap();

        let result = tracker.get_active_deployment();
        let err = result.unwrap_err();
        assert!(
            matches!(&err, DeploymentTrackerError::Io(e) if e.kind() != std::io::ErrorKind::NotFound)
        );
    }

    #[test]
    fn get_active_deployment_invalid_filename() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let dir = TempDir::new().unwrap();
        let tracker_dir = dir.path().join("tracker");
        std::fs::create_dir_all(&tracker_dir).unwrap();

        // Create a file with invalid UTF-8 name
        let invalid_name = OsStr::from_bytes(&[0xff, 0xfe]);
        let file_path = tracker_dir.join(invalid_name);
        std::fs::write(&file_path, "cmd-1").unwrap();

        let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(tracker_dir);

        let result = tracker.get_active_deployment();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid file name"));
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;
    use crate::system::SystemFileOperations;
    use tempfile::TempDir;

    #[test]
    fn get_active_deployment_skips_subdirectories() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-1", "cmd-1").unwrap();
        // Create a subdirectory — should be skipped (continue branch)
        std::fs::create_dir(dir.path().join("subdir")).unwrap();

        let active = tracker.get_active_deployment().unwrap().unwrap();
        assert_eq!(active.deployment_id, "d-1");
    }

    #[test]
    fn get_active_deployment_keeps_newer_file() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        tracker.start_tracking("d-1", "cmd-1").unwrap();
        tracker.start_tracking("d-2", "cmd-2").unwrap();

        // Use explicit timestamps to avoid sleep-based flakiness
        // Use recent times so files aren't considered stale (>24h)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        #[allow(clippy::cast_possible_wrap)]
        let now = now as i64;
        let t_older = filetime::FileTime::from_unix_time(now - 60, 0);
        let t_newer = filetime::FileTime::from_unix_time(now, 0);
        filetime::set_file_mtime(dir.path().join("d-2"), t_older).unwrap();
        filetime::set_file_mtime(dir.path().join("d-1"), t_newer).unwrap();

        let active = tracker.get_active_deployment().unwrap().unwrap();
        assert_eq!(active.deployment_id, "d-1");
    }
}
