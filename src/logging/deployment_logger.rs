//! @risk low
//!
//! Per-deployment log file writer.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use chrono::Local;

use super::LogConfig;

const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024; // 64 MB
const MAX_FILES: usize = 8;

/// Per-deployment log file mode for the given policy: 0644 by default
/// (world-readable — non-root log collectors ship this file), 0640 under
/// opt-in hardening, since script output can carry customer-sensitive data.
#[cfg(unix)]
pub(crate) fn log_file_mode(restrict: bool) -> u32 {
    if restrict { 0o640 } else { 0o644 }
}

/// Deployment-specific logger that writes to a separate log file.
///
/// Per-deployment log file:
/// - Size-based rotation: 64 MB per file, 8 files kept
/// - Format: `[{timestamp}] {message}`
#[derive(Debug)]
pub struct DeploymentLogger {
    path: PathBuf,
    file: File,
    restrict_permissions: bool,
}

impl DeploymentLogger {
    /// Creates a new deployment logger, creating the log directory if needed.
    ///
    /// Dir/file modes follow `LogConfig::restrict_permissions`: 0755/0644 by
    /// default, since non-root log collectors read the deployment log the same
    /// way they read the deployment-root dirs; 0750/0640 under the opt-in
    /// `restrict_agent_dir_permissions` hardening (script output can carry
    /// customer-sensitive data).
    ///
    /// # Errors
    ///
    /// Returns an error if the log directory or file cannot be created.
    pub fn new(config: &LogConfig) -> io::Result<Self> {
        let dir = config.root_dir.join("deployment-logs");
        crate::system::create_deployment_dir(&dir, 0o750, config.restrict_permissions)?;

        let path = dir.join(format!("{}-deployments.log", config.program_name));
        let file = open_log_file(&path, config.restrict_permissions)?;

        Ok(Self { path, file, restrict_permissions: config.restrict_permissions })
    }

    /// Logs a deployment event message.
    ///
    /// # Errors
    ///
    /// Returns an error if the log file cannot be written to or rotated.
    pub fn log(&mut self, message: &str) -> io::Result<()> {
        self.rotate_if_needed()?;
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
        writeln!(self.file, "[{ts}] {message}")
    }

    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let size = self.file.metadata()?.len();
        if size < MAX_FILE_SIZE {
            return Ok(());
        }

        // Rotate: .7 → delete, .6 → .7, ... .1 → .2, current → .1
        for i in (1..MAX_FILES).rev() {
            let from = rotated_path(&self.path, i);
            let to = rotated_path(&self.path, i + 1);
            if from.exists() {
                if i + 1 >= MAX_FILES {
                    fs::remove_file(&from)?;
                } else {
                    fs::rename(&from, &to)?;
                }
            }
        }

        let first_rotated = rotated_path(&self.path, 1);
        fs::rename(&self.path, &first_rotated)?;

        self.file = open_log_file(&self.path, self.restrict_permissions)?;

        Ok(())
    }
}

/// Open (or create) the deployment log file with the policy mode.
/// Unix: 0644 default, 0640 restricted. The mode is force-set
/// on every open so a file a tightened agent left 0640 heals to 0644 on the
/// next deployment (and converges to 0640 if the flag is turned on).
/// Other platforms: platform defaults.
fn open_log_file(path: &Path, restrict: bool) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = log_file_mode(restrict);
        let f = OpenOptions::new().create(true).append(true).mode(mode).open(path)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        Ok(f)
    }
    #[cfg(not(unix))]
    {
        let _ = restrict;
        OpenOptions::new().create(true).append(true).open(path)
    }
}

fn rotated_path(base: &Path, index: usize) -> PathBuf {
    let name = base.file_name().expect("log path must have a filename").to_string_lossy();
    base.with_file_name(format!("{name}.{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(dir: &Path) -> LogConfig {
        LogConfig {
            log_dir: dir.to_path_buf(),
            verbose: false,
            program_name: "test-agent".to_string(),
            root_dir: dir.to_path_buf(),
            restrict_permissions: false,
            restrict_log_permissions: false,
        }
    }

    #[test]
    fn creates_deployment_log_directory_and_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let logger = DeploymentLogger::new(&config).unwrap();

        assert!(dir.path().join("deployment-logs").exists());
        assert!(logger.path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn default_creates_world_readable_dir_and_file() {
        // Non-root log collectors ship the deployment log, so the dir must be
        // 0755 and the file 0644 by default.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let logger = DeploymentLogger::new(&config).unwrap();

        let dir_mode =
            fs::metadata(dir.path().join("deployment-logs")).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&logger.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o755, "deployment-logs dir mode {dir_mode:#o}, want 0755");
        assert_eq!(file_mode, 0o644, "deployment log file mode {file_mode:#o}, want 0644");
    }

    #[cfg(unix)]
    #[test]
    fn restricted_creates_hardened_dir_and_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let mut config = test_config(dir.path());
        config.restrict_permissions = true;
        let logger = DeploymentLogger::new(&config).unwrap();

        let dir_mode =
            fs::metadata(dir.path().join("deployment-logs")).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&logger.path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o750, "deployment-logs dir mode {dir_mode:#o}, want 0750");
        assert_eq!(file_mode, 0o640, "deployment log file mode {file_mode:#o}, want 0640");
    }

    #[cfg(unix)]
    #[test]
    fn default_loosens_file_tightened_by_previous_agent() {
        // Upgrade path: a tightened 2.0.0 agent left the file 0640; the next
        // open under default policy must heal it to 0644.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let logs_dir = dir.path().join("deployment-logs");
        fs::create_dir_all(&logs_dir).unwrap();
        let path = logs_dir.join("test-agent-deployments.log");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&logs_dir, fs::Permissions::from_mode(0o750)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let config = test_config(dir.path());
        let _logger = DeploymentLogger::new(&config).unwrap();

        let dir_mode = fs::metadata(&logs_dir).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o755, "expected 0750 -> 0755 heal, got {dir_mode:#o}");
        assert_eq!(file_mode, 0o644, "expected 0640 -> 0644 heal, got {file_mode:#o}");
    }

    #[test]
    fn writes_timestamped_messages() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let mut logger = DeploymentLogger::new(&config).unwrap();

        logger.log("deployment started").unwrap();
        logger.log("deployment finished").unwrap();

        let content = fs::read_to_string(&logger.path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("deployment started"));
        assert!(lines[1].contains("deployment finished"));
        // Verify timestamp format: [YYYY-MM-DD HH:MM:SS.mmm]
        assert!(lines[0].starts_with('['));
        assert!(lines[0].contains(']'));
    }

    #[test]
    fn rotated_path_appends_index() {
        let base = PathBuf::from("/var/log/test.log");
        assert_eq!(rotated_path(&base, 1), PathBuf::from("/var/log/test.log.1"));
        assert_eq!(rotated_path(&base, 7), PathBuf::from("/var/log/test.log.7"));
    }

    #[test]
    fn skips_rotation_when_under_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let mut logger = DeploymentLogger::new(&config).unwrap();

        logger.log("small message").unwrap();

        // No rotated files should exist
        let rotated = rotated_path(&logger.path, 1);
        assert!(!rotated.exists());
    }

    #[test]
    fn rotates_when_file_exceeds_max_size() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let mut logger = DeploymentLogger::new(&config).unwrap();

        // Write enough data to exceed MAX_FILE_SIZE (64 MB)
        // We'll use a smaller approach: write to the file directly to simulate size
        let big_data = "x".repeat(1024 * 1024); // 1 MB chunk
        for _ in 0..65 {
            logger.file.write_all(big_data.as_bytes()).unwrap();
        }
        logger.file.flush().unwrap();

        // Next log call should trigger rotation
        logger.log("after rotation").unwrap();

        let rotated = rotated_path(&logger.path, 1);
        assert!(rotated.exists(), "rotated file .1 should exist");
        assert!(logger.path.exists(), "new current file should exist");

        let content = fs::read_to_string(&logger.path).unwrap();
        assert!(content.contains("after rotation"));
    }

    #[test]
    fn rotation_deletes_oldest_when_at_max_files() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path());
        let logger = DeploymentLogger::new(&config).unwrap();

        // Pre-create rotated files .1 through .7
        for i in 1..MAX_FILES {
            let path = rotated_path(&logger.path, i);
            fs::write(&path, format!("old-{i}")).unwrap();
        }

        drop(logger);
        let mut logger = DeploymentLogger::new(&config).unwrap();

        // Fill current file past limit
        let big_data = "x".repeat(1024 * 1024);
        for _ in 0..65 {
            logger.file.write_all(big_data.as_bytes()).unwrap();
        }
        logger.file.flush().unwrap();

        logger.log("newest").unwrap();

        // .7 should now contain what was in .6 (old .7 deleted)
        let content_7 = fs::read_to_string(rotated_path(&logger.path, 7)).unwrap();
        assert_eq!(content_7, "old-6");

        // .1 should be the big rotated file
        assert!(rotated_path(&logger.path, 1).exists());

        // Current file should have the newest message
        let current = fs::read_to_string(&logger.path).unwrap();
        assert!(current.contains("newest"));
    }

    #[test]
    fn log_file_uses_program_name() {
        let dir = tempfile::tempdir().unwrap();
        let config = LogConfig {
            log_dir: dir.path().to_path_buf(),
            verbose: false,
            program_name: "my-agent".to_string(),
            root_dir: dir.path().to_path_buf(),
            restrict_permissions: false,
            restrict_log_permissions: false,
        };
        let logger = DeploymentLogger::new(&config).unwrap();

        assert!(logger.path.file_name().unwrap().to_string_lossy().contains("my-agent"));
    }
}
