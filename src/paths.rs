//! @risk low
//!
//! Platform-appropriate default paths for the `CodeDeploy` agent.
//!
//! Each function returns the canonical default path for the current platform,
//! as described by the agent's documented install layout: the standard Linux
//! system directories on Unix, and paths based under
//! `%PROGRAMDATA%\Amazon\CodeDeploy` on Windows.

use std::path::PathBuf;

/// Leading separators to strip from an `AppSpec` path (`files: source`,
/// `hooks: location`) before resolving it against the deployment archive.
///
/// `\` is a separator on Windows only; on Unix it is a legal filename character.
pub const APPSPEC_PATH_SEPARATORS: &[char] = if cfg!(windows) { &['/', '\\'] } else { &['/'] };

/// Returns the Windows base directory: `%PROGRAMDATA%\Amazon\CodeDeploy`.
///
/// Read from `%PROGRAMDATA%`, falling back to `C:\ProgramData`.
#[cfg(windows)]
fn windows_base() -> PathBuf {
    use std::path::Path;
    const DEFAULT_PROGRAM_DATA: &str = r"C:\ProgramData";

    // Accept %PROGRAMDATA% only if non-empty and absolute, else use the default:
    // a relative or empty value would resolve the agent's whole directory tree
    // against the process CWD.
    let program_data = std::env::var("PROGRAMDATA")
        .ok()
        .filter(|p| !p.trim().is_empty() && Path::new(p).is_absolute())
        .unwrap_or_else(|| DEFAULT_PROGRAM_DATA.to_string());
    PathBuf::from(program_data).join(r"Amazon\CodeDeploy")
}

/// Default agent config file path.
///
/// - Unix: `/etc/codedeploy-agent/conf/codedeployagent.yml`
/// - Windows: `%PROGRAMDATA%\Amazon\CodeDeploy\conf.yml`
///
/// On Windows `conf.yml` lives directly under the install root.
#[must_use]
pub fn config_file() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/etc/codedeploy-agent/conf/codedeployagent.yml")
    }
    #[cfg(windows)]
    {
        windows_base().join("conf.yml")
    }
}

/// Default on-premises config file path.
///
/// - Unix: `/etc/codedeploy-agent/conf/codedeploy.onpremises.yml`
/// - Windows: `%PROGRAMDATA%\Amazon\CodeDeploy\conf.onpremises.yml`
#[must_use]
pub fn on_premises_config_file() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/etc/codedeploy-agent/conf/codedeploy.onpremises.yml")
    }
    #[cfg(windows)]
    {
        windows_base().join("conf.onpremises.yml")
    }
}

/// Default log directory.
///
/// - Unix: `/var/log/aws/codedeploy-agent`
/// - Windows: `%PROGRAMDATA%\Amazon\CodeDeploy\log`
#[must_use]
pub fn log_dir() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/var/log/aws/codedeploy-agent")
    }
    #[cfg(windows)]
    {
        windows_base().join("log")
    }
}

/// Default PID directory.
///
/// - Unix: `/opt/codedeploy-agent/state/.pid`
/// - Windows: `%PROGRAMDATA%\Amazon\CodeDeploy\state`
///
/// On Windows the agent runs as a Windows Service (no PID file), but this
/// path is still provided so shared code paths compile without `cfg` guards.
#[must_use]
pub fn pid_dir() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/opt/codedeploy-agent/state/.pid")
    }
    #[cfg(windows)]
    {
        windows_base().join("state")
    }
}

/// Default deployment root directory.
///
/// - Unix: `/opt/codedeploy-agent/deployment-root`
/// - Windows: `%PROGRAMDATA%\Amazon\CodeDeploy`
///
/// The Windows install layout puts the deployment root directly at
/// `%PROGRAMDATA%\Amazon\CodeDeploy` — there is no `deployment-root` suffix
/// there, unlike Unix.
#[must_use]
pub fn root_dir() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/opt/codedeploy-agent/deployment-root")
    }
    #[cfg(windows)]
    {
        windows_base()
    }
}

/// Path to the agent version-tracking file (`.version`).
///
/// - Unix: `/opt/codedeploy-agent/.version`
/// - Windows: `%PROGRAMDATA%\Amazon\CodeDeploy\.version`
#[must_use]
pub fn version_file() -> PathBuf {
    #[cfg(unix)]
    {
        PathBuf::from("/opt/codedeploy-agent/.version")
    }
    #[cfg(windows)]
    {
        windows_base().join(".version")
    }
}

/// Path for the updater log.
///
/// - Unix: `/var/log/aws/codedeploy-agent/codedeploy-agent-updater.log`
/// - Windows: `%PROGRAMDATA%\Amazon\CodeDeploy\log\codedeploy-agent-updater-log.txt`
///
/// SECURITY: the updater log goes to the agent-owned log directory rather than
/// a world-writable location such as `/tmp`, which would expose it to
/// symlink-based TOCTOU attacks (CWE-59/CWE-367).
#[must_use]
pub fn updater_log_path() -> PathBuf {
    #[cfg(unix)]
    {
        log_dir().join("codedeploy-agent-updater.log")
    }
    #[cfg(windows)]
    {
        log_dir().join("codedeploy-agent-updater-log.txt")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn appspec_separators_exclude_backslash_on_unix() {
        assert_eq!(APPSPEC_PATH_SEPARATORS, &['/']);
        assert_eq!(r"\app".trim_start_matches(APPSPEC_PATH_SEPARATORS), r"\app");
    }

    #[cfg(windows)]
    #[test]
    fn appspec_separators_include_backslash_on_windows() {
        assert_eq!(APPSPEC_PATH_SEPARATORS, &['/', '\\']);
        assert_eq!(r"\app".trim_start_matches(APPSPEC_PATH_SEPARATORS), "app");
        assert_eq!(r"\".trim_start_matches(APPSPEC_PATH_SEPARATORS), "");
    }

    #[test]
    fn appspec_separators_strip_repeated_slashes() {
        assert_eq!("//app".trim_start_matches(APPSPEC_PATH_SEPARATORS), "app");
        assert_eq!("/".trim_start_matches(APPSPEC_PATH_SEPARATORS), "");
    }

    #[test]
    fn appspec_separators_leave_relative_paths_alone() {
        assert_eq!("app/sub".trim_start_matches(APPSPEC_PATH_SEPARATORS), "app/sub");
        assert_eq!("./app".trim_start_matches(APPSPEC_PATH_SEPARATORS), "./app");
    }

    #[cfg(unix)]
    #[test]
    fn config_file_returns_unix_path() {
        assert_eq!(config_file(), PathBuf::from("/etc/codedeploy-agent/conf/codedeployagent.yml"));
    }

    #[cfg(unix)]
    #[test]
    fn on_premises_config_file_returns_unix_path() {
        assert_eq!(
            on_premises_config_file(),
            PathBuf::from("/etc/codedeploy-agent/conf/codedeploy.onpremises.yml")
        );
    }

    #[cfg(unix)]
    #[test]
    fn log_dir_returns_unix_path() {
        assert_eq!(log_dir(), PathBuf::from("/var/log/aws/codedeploy-agent"));
    }

    #[cfg(unix)]
    #[test]
    fn pid_dir_returns_unix_path() {
        assert_eq!(pid_dir(), PathBuf::from("/opt/codedeploy-agent/state/.pid"));
    }

    #[cfg(unix)]
    #[test]
    fn root_dir_returns_unix_path() {
        assert_eq!(root_dir(), PathBuf::from("/opt/codedeploy-agent/deployment-root"));
    }

    #[cfg(unix)]
    #[test]
    fn updater_log_path_returns_unix_path() {
        assert_eq!(
            updater_log_path(),
            PathBuf::from("/var/log/aws/codedeploy-agent/codedeploy-agent-updater.log")
        );
    }
}
