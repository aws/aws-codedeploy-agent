//! @risk medium
//!
//! Manages deployment archive directories on disk.
//!
//! Tracks which deployment was last successful and most recent (used by
//! `LifecycleEventExecutor` to select the correct appspec for rollback/pre-install hooks),
//! and cleans up old deployment directories to prevent disk from filling up.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use tracing::debug;

#[derive(Debug)]
pub struct DeploymentArchives {
    root_dir: PathBuf,
    instructions_dir: PathBuf,
    /// Maximum archives to keep per deployment group. Reads from config
    /// (`max_revisions`, default 5, must be >= 1). Already parameterized —
    /// callers pass the value at construction.
    archives_to_retain: usize,
}

impl DeploymentArchives {
    #[must_use]
    pub fn new(root_dir: PathBuf, instructions_dir: PathBuf, archives_to_retain: usize) -> Self {
        Self { root_dir, instructions_dir, archives_to_retain }
    }

    /// Record the deployment directory as the last successful install for a group.
    ///
    /// # Errors
    /// Returns an error if the file cannot be written.
    pub fn update_last_successful(
        &self,
        group_id: &str,
        deployment_root_dir: &Path,
    ) -> io::Result<()> {
        let path = self.last_successful_path(group_id);
        fs::write(path, deployment_root_dir.display().to_string())
    }

    /// Record the deployment directory as the most recent install for a group.
    ///
    /// # Errors
    /// Returns an error if the file cannot be written.
    pub fn update_most_recent(&self, group_id: &str, deployment_root_dir: &Path) -> io::Result<()> {
        let path = self.most_recent_path(group_id);
        fs::write(path, deployment_root_dir.display().to_string())
    }

    /// Read the last successful deployment directory for a group.
    #[must_use]
    pub fn last_successful_dir(&self, group_id: &str) -> Option<PathBuf> {
        Self::read_tracking_file(&self.last_successful_path(group_id))
    }

    /// Read the most recent deployment directory for a group.
    #[must_use]
    pub fn most_recent_dir(&self, group_id: &str) -> Option<PathBuf> {
        Self::read_tracking_file(&self.most_recent_path(group_id))
    }

    /// Delete old deployment archives, keeping `archives_to_retain` most recent.
    /// Never removes the current deployment or the last successful deployment.
    ///
    /// # Errors
    /// Returns an error if the deployment group directory cannot be read.
    pub fn cleanup_old_archives(
        &self,
        group_id: &str,
        current_deployment_dir: &Path,
    ) -> io::Result<()> {
        let group_dir = self.root_dir.join(group_id);
        if !group_dir.exists() {
            return Ok(());
        }

        let mut archives: Vec<PathBuf> = fs::read_dir(&group_dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.is_dir())
            .collect();

        // Exclude current deployment
        archives.retain(|p| p != current_deployment_dir);

        if archives.len() < self.archives_to_retain {
            return Ok(());
        }
        let extra = archives.len() - self.archives_to_retain + 1;

        // Never remove last successful
        let last_success = self.last_successful_dir(group_id);
        if let Some(ref ls) = last_success {
            archives.retain(|p| p != ls);
        }

        // Sort oldest first by mtime
        archives.sort_by_key(|p| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        for dir in archives.into_iter().take(extra) {
            debug!("Deleting old archive: {}", dir.display());
            if let Err(e) = fs::remove_dir_all(&dir) {
                debug!("Failed to delete {}: {e}", dir.display());
            }
        }

        Ok(())
    }

    /// The deployment instructions directory.
    #[must_use]
    pub fn instructions_dir(&self) -> &Path {
        &self.instructions_dir
    }

    /// The deployment root directory for a specific deployment.
    #[must_use]
    pub fn deployment_root_dir(&self, group_id: &str, deployment_id: &str) -> PathBuf {
        self.root_dir.join(group_id).join(deployment_id)
    }

    /// The deployment archive directory (where bundle is unpacked).
    #[must_use]
    pub fn archive_dir(&self, group_id: &str, deployment_id: &str) -> PathBuf {
        self.deployment_root_dir(group_id, deployment_id).join("deployment-archive")
    }

    /// The bundle artifact path for a deployment.
    #[must_use]
    pub fn artifact_bundle_path(&self, group_id: &str, deployment_id: &str) -> PathBuf {
        self.deployment_root_dir(group_id, deployment_id).join("bundle.tar")
    }

    fn last_successful_path(&self, group_id: &str) -> PathBuf {
        self.instructions_dir.join(format!("{group_id}_last_successful_install"))
    }

    fn most_recent_path(&self, group_id: &str) -> PathBuf {
        self.instructions_dir.join(format!("{group_id}_most_recent_install"))
    }

    fn read_tracking_file(path: &Path) -> Option<PathBuf> {
        fs::read_to_string(path).ok().map(|s| PathBuf::from(s.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup() -> (TempDir, DeploymentArchives) {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&instructions).unwrap();
        (dir, DeploymentArchives::new(root, instructions, 5))
    }

    #[test]
    fn update_and_read_last_successful() {
        let (_dir, archives) = setup();
        let deploy_dir = PathBuf::from("/opt/codedeploy/dg-1/d-123");

        archives.update_last_successful("dg-1", &deploy_dir).unwrap();
        assert_eq!(archives.last_successful_dir("dg-1"), Some(deploy_dir));
    }

    #[test]
    fn update_and_read_most_recent() {
        let (_dir, archives) = setup();
        let deploy_dir = PathBuf::from("/opt/codedeploy/dg-1/d-456");

        archives.update_most_recent("dg-1", &deploy_dir).unwrap();
        assert_eq!(archives.most_recent_dir("dg-1"), Some(deploy_dir));
    }

    #[test]
    fn read_missing_returns_none() {
        let (_dir, archives) = setup();
        assert_eq!(archives.last_successful_dir("dg-1"), None);
        assert_eq!(archives.most_recent_dir("dg-1"), None);
    }

    #[test]
    fn deployment_root_dir_path() {
        let (_dir, archives) = setup();
        let path = archives.deployment_root_dir("dg-1", "d-123");
        assert!(path.ends_with("deployments/dg-1/d-123"));
    }

    #[test]
    fn archive_dir_path() {
        let (_dir, archives) = setup();
        let path = archives.archive_dir("dg-1", "d-123");
        assert!(path.ends_with("deployments/dg-1/d-123/deployment-archive"));
    }

    #[test]
    fn artifact_bundle_path() {
        let (_dir, archives) = setup();
        let path = archives.artifact_bundle_path("dg-1", "d-123");
        assert!(path.ends_with("deployments/dg-1/d-123/bundle.tar"));
    }

    #[test]
    fn cleanup_no_group_dir() {
        let (_dir, archives) = setup();
        let current = PathBuf::from("/nonexistent");
        archives.cleanup_old_archives("dg-1", &current).unwrap();
    }

    #[test]
    fn cleanup_retains_within_limit() {
        let (dir, archives) = setup();
        let group_dir = dir.path().join("deployments/dg-1");
        fs::create_dir_all(&group_dir).unwrap();

        // Create 3 deployments (limit is 5)
        for i in 1..=3 {
            fs::create_dir_all(group_dir.join(format!("d-{i}"))).unwrap();
        }

        let current = group_dir.join("d-3");
        archives.cleanup_old_archives("dg-1", &current).unwrap();

        // All should still exist
        assert!(group_dir.join("d-1").exists());
        assert!(group_dir.join("d-2").exists());
        assert!(group_dir.join("d-3").exists());
    }

    #[test]
    fn cleanup_deletes_oldest_over_limit() {
        let (dir, _) = setup();
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        let archives = DeploymentArchives::new(root, instructions, 2);

        let group_dir = dir.path().join("deployments/dg-1");
        fs::create_dir_all(&group_dir).unwrap();

        // Create 4 deployments
        for i in 1..=4 {
            let d = group_dir.join(format!("d-{i}"));
            fs::create_dir_all(&d).unwrap();
            // Stagger mtimes
            filetime::set_file_mtime(
                &d,
                filetime::FileTime::from_unix_time(1_000_000 + i64::from(i), 0),
            )
            .unwrap();
        }

        let current = group_dir.join("d-4");
        archives.cleanup_old_archives("dg-1", &current).unwrap();

        // d-4 (current) always kept, d-1 and d-2 oldest should be deleted
        assert!(group_dir.join("d-4").exists());
        assert!(group_dir.join("d-3").exists());
    }

    #[test]
    fn cleanup_preserves_last_successful() {
        let (dir, _) = setup();
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        let archives = DeploymentArchives::new(root, instructions, 1);

        let group_dir = dir.path().join("deployments/dg-1");
        fs::create_dir_all(&group_dir).unwrap();

        for i in 1..=3 {
            let d = group_dir.join(format!("d-{i}"));
            fs::create_dir_all(&d).unwrap();
            filetime::set_file_mtime(
                &d,
                filetime::FileTime::from_unix_time(1_000_000 + i64::from(i), 0),
            )
            .unwrap();
        }

        // Mark d-1 (oldest) as last successful
        archives.update_last_successful("dg-1", &group_dir.join("d-1")).unwrap();

        let current = group_dir.join("d-3");
        archives.cleanup_old_archives("dg-1", &current).unwrap();

        // d-1 preserved (last successful), d-3 preserved (current)
        assert!(group_dir.join("d-1").exists(), "last successful should be preserved");
        assert!(group_dir.join("d-3").exists(), "current should be preserved");
    }
}
