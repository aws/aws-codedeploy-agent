//! @risk low
//!
//! Per-deployment log file writer.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::Local;

use super::LogConfig;

const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024; // 64 MB
const MAX_FILES: usize = 8;

/// Deployment-specific logger that writes to a separate log file.
///
/// Per-deployment log file:
/// - Size-based rotation: 64 MB per file, 8 files kept
/// - Format: `[{timestamp}] {message}`
#[derive(Debug)]
pub struct DeploymentLogger {
    path: PathBuf,
    file: File,
}

impl DeploymentLogger {
    /// Creates a new deployment logger, creating the log directory if needed.
    ///
    /// # Errors
    ///
    /// Returns an error if the log directory or file cannot be created.
    pub fn new(config: &LogConfig) -> io::Result<Self> {
        let dir = config.root_dir.join("deployment-logs");
        fs::create_dir_all(&dir)?;

        let path = dir.join(format!("{}-deployments.log", config.program_name));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;

        Ok(Self { path, file })
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

        self.file = OpenOptions::new().create(true).append(true).open(&self.path)?;

        Ok(())
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
        };
        let logger = DeploymentLogger::new(&config).unwrap();

        assert!(logger.path.file_name().unwrap().to_string_lossy().contains("my-agent"));
    }
}
