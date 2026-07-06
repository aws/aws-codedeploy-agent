//! Logging infrastructure.
mod agent_logger;
mod deployment_logger;

pub use agent_logger::LogGuard;
pub use deployment_logger::DeploymentLogger;

use std::path::PathBuf;

use crate::paths;

/// Logging configuration resolved from the agent config.
#[derive(Debug, Clone)]
pub struct LogConfig {
    pub log_dir: PathBuf,
    pub verbose: bool,
    pub program_name: String,
    pub root_dir: PathBuf,
    /// Mirror of `AgentConfig::restrict_agent_dir_permissions`: when
    /// `true`, the deployment-logs dir/files use hardened 0750/0640 instead
    /// of the default 0755/0644.
    pub restrict_permissions: bool,
    /// Mirror of `AgentConfig::restrict_log_dir_permissions`: when `true`,
    /// the agent log dir/files (`log_dir`) use hardened 0750/0640 instead of
    /// the default world-readable 0755/0644. Breaks non-root log collectors.
    pub restrict_log_permissions: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            log_dir: paths::log_dir(),
            verbose: false,
            program_name: "codedeploy-agent".to_string(),
            root_dir: paths::root_dir(),
            restrict_permissions: false,
            restrict_log_permissions: false,
        }
    }
}

/// Returns the platform-appropriate updater log path.
///
/// Writes to the agent-owned log directory rather than `/tmp` to avoid
/// symlink-based TOCTOU attacks on world-writable paths.
#[must_use]
pub fn updater_log_path() -> PathBuf {
    paths::updater_log_path()
}

/// Initializes the agent logging system.
///
/// Returns a [`LogGuard`] that must be held for the lifetime of the process.
/// Dropping the guard flushes and shuts down the non-blocking writer.
///
/// # Errors
///
/// Returns an error if the log directory cannot be created.
// GRCOV_STOP_COVERAGE
pub fn init_logging(config: &LogConfig) -> std::io::Result<LogGuard> {
    agent_logger::init(config)
}
// GRCOV_BEGIN_COVERAGE

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

    #[cfg(not(target_os = "windows"))]
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
    fn updater_log_path_is_under_agent_log_dir() {
        assert_eq!(
            updater_log_path(),
            PathBuf::from("/var/log/aws/codedeploy-agent/codedeploy-agent-updater.log")
        );
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
