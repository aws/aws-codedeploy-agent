//! @risk low
//!
//! Logging infrastructure.
mod agent_logger;
mod deployment_logger;

pub use agent_logger::LogGuard;
pub use deployment_logger::DeploymentLogger;

use std::path::PathBuf;

/// Temporary logging configuration until T2.1 (full YAML config parsing) lands.
#[derive(Debug, Clone)]
pub struct LogConfig {
    pub log_dir: PathBuf,
    pub verbose: bool,
    pub program_name: String,
    pub root_dir: PathBuf,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_dir: default_log_dir(),
            verbose: false,
            program_name: "codedeploy-agent".to_string(),
            root_dir: PathBuf::from("/opt/codedeploy-agent/deployment-root"),
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn default_log_dir() -> PathBuf {
    PathBuf::from("/var/log/aws/codedeploy-agent")
}

#[cfg(target_os = "windows")]
fn default_log_dir() -> PathBuf {
    PathBuf::from(r"C:\ProgramData\Amazon\CodeDeploy\log")
}

/// Path for the updater log.
/// TODO(T11): Wire this into the agent self-update flow.
#[cfg(not(target_os = "windows"))]
pub const UPDATER_LOG_PATH: &str = "/tmp/codedeploy-agent.update.log";

/// Path for the updater log on Windows.
/// TODO(T11): Wire this into the agent self-update flow.
#[cfg(target_os = "windows")]
pub const UPDATER_LOG_PATH: &str =
    r"C:\ProgramData\Amazon\CodeDeploy\log\codedeploy-agent-updater-log.txt";

/// Initializes the agent logging system.
///
/// Returns a [`LogGuard`] that must be held for the lifetime of the process.
/// Dropping the guard flushes and shuts down the non-blocking writer.
///
/// # Errors
///
/// Returns an error if the log directory cannot be created.
pub fn init_logging(config: &LogConfig) -> std::io::Result<LogGuard> {
    agent_logger::init(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_program_name() {
        let config = LogConfig::default();
        assert_eq!(config.program_name, "codedeploy-agent");
    }

    #[test]
    fn default_config_is_not_verbose() {
        let config = LogConfig::default();
        assert!(!config.verbose);
    }

    #[test]
    fn default_config_has_standard_root_dir() {
        let config = LogConfig::default();
        assert_eq!(config.root_dir, PathBuf::from("/opt/codedeploy-agent/deployment-root"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn default_log_dir_is_linux_path() {
        let config = LogConfig::default();
        assert_eq!(config.log_dir, PathBuf::from("/var/log/aws/codedeploy-agent"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn updater_log_path_is_tmp() {
        assert_eq!(UPDATER_LOG_PATH, "/tmp/codedeploy-agent.update.log");
    }

    #[test]
    fn log_config_is_cloneable() {
        let config = LogConfig::default();
        let cloned = config.clone();
        assert_eq!(config.program_name, cloned.program_name);
        assert_eq!(config.verbose, cloned.verbose);
    }

    #[test]
    fn log_config_is_debuggable() {
        let config = LogConfig::default();
        let debug_str = format!("{config:?}");
        assert!(debug_str.contains("codedeploy-agent"));
    }
}
