//! @risk medium
//!
//! Script execution with real-time stream logging.
//!
//! [`Script::execute`] is the sync entry point — it creates a tokio runtime internally
//! and delegates to [`execute_async`](Script::execute_async).
//!
//! ## Design notes
//!
//! - **Process group isolation**: `process_group(0)` puts the child in its own group so
//!   the kill targets the script and any children it spawned, not the agent itself.
//! - **SIGTERM on timeout**: Sends SIGTERM to the process group, giving scripts a chance
//!   to clean up before being killed.
//! - **Real-time streaming**: stdout/stderr are streamed line-by-line to the shared log
//!   so users can `tail -f` the deployment log during execution.

use super::script_run_log::ScriptRunLog;
use crate::system::process_ops::kill_process_group;
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;
use tracing::{debug, info};

/// A deployment lifecycle script to be executed.
#[derive(Debug)]
pub struct Script {
    path: PathBuf,
    runas: Option<String>,
    sudo: bool,
    env_vars: HashMap<String, String>,
    log: Arc<Mutex<ScriptRunLog>>,
}

impl Script {
    #[must_use]
    pub fn new<S: BuildHasher>(
        path: PathBuf,
        runas: Option<String>,
        sudo: bool,
        env_vars: &HashMap<String, String, S>,
        log: Arc<Mutex<ScriptRunLog>>,
    ) -> Self {
        Self {
            path,
            runas,
            sudo,
            env_vars: env_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            log,
        }
    }

    /// Sync entry point. Spawns the script, streams output, waits up to `timeout`.
    ///
    /// Caller is responsible for pre-flight checks (existence, executable permission).
    ///
    /// # Errors
    /// Returns an error if the script fails to spawn, exceeds the timeout,
    /// or the tokio runtime cannot be created.
    pub fn execute(self, timeout: Duration) -> Result<i32, String> {
        info!(
            script = %self.path.display(),
            timeout_secs = timeout.as_secs(),
            runas = ?self.runas,
            "Executing script"
        );

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;

        rt.block_on(self.execute_async(timeout))
    }

    /// Async implementation. Spawns the script, streams stdout/stderr to the
    /// shared log in real-time, and waits with a shared deadline.
    ///
    /// Uses [`DeadlineJoiner`] to match Ruby's `ThreadJoiner` pattern:
    /// 1. Wait for process exit (up to deadline) — timeout → kill + `"timeout"`
    /// 2. Wait for stdout to close (remaining time) — timeout → `"outputs_left_open"`
    /// 3. Wait for stderr to close (remaining time) — timeout → `"outputs_left_open"`
    ///
    /// Ruby ref: `hook_executor.rb` `execute_script` method.
    ///
    /// # Errors
    /// Returns `"timeout"` if the process exceeds the deadline, or
    /// `"outputs_left_open"` if the process exits but stdout/stderr remain open.
    pub async fn execute_async(self, timeout: Duration) -> Result<i32, String> {
        let mut cmd = build_command(&self.path, self.runas.as_deref(), self.sudo);
        cmd.envs(&self.env_vars)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let joiner = super::deadline_joiner::DeadlineJoiner::new(timeout);

        // Phase 1: wait for process exit (Ruby: joiner.joinOrFail(wait_thr))
        let stdout_log = Arc::clone(&self.log);
        let stderr_log = Arc::clone(&self.log);
        let stdout_handle =
            tokio::spawn(async move { stream_to_log(stdout, &stdout_log, "[stdout]").await });
        let stderr_handle =
            tokio::spawn(async move { stream_to_log(stderr, &stderr_log, "[stderr]").await });

        let status = match joiner.join(child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(e.to_string()),
            Err(()) => {
                kill_process_group(pid);
                return Err("timeout".to_string());
            },
        };

        // Phase 2: wait for stdout to close (Ruby: joiner.joinOrFail(stdout_thread))
        if joiner.join(stdout_handle).await.is_err() {
            return Err("outputs_left_open".to_string());
        }

        // Phase 3: wait for stderr to close (Ruby: joiner.joinOrFail(stderr_thread))
        if joiner.join(stderr_handle).await.is_err() {
            return Err("outputs_left_open".to_string());
        }

        let code = status.code().unwrap_or(1);
        if code != 0 {
            log_process_diagnostics(status, pid);
        }

        Ok(code)
    }
}

/// Build the command for script execution.
///
/// | `runas` | `sudo` | Command                              |
/// |---------|--------|--------------------------------------|
/// | Some    | true   | `sudo su <user> -c <script>`         |
/// | Some    | false  | `su <user> -c <script>`              |
/// | None    | true   | `sudo <script>`                      |
/// | None    | false  | `<script>` (run as agent user)       |
pub(super) fn build_command(script_path: &Path, runas: Option<&str>, sudo: bool) -> Command {
    match (runas, sudo) {
        (Some(user), true) => {
            let mut cmd = Command::new("sudo");
            cmd.args(["su", user, "-c", &script_path.display().to_string()]);
            cmd
        },
        (Some(user), false) => {
            let mut cmd = Command::new("su");
            cmd.args([user, "-c", &script_path.display().to_string()]);
            cmd
        },
        (None, true) => {
            let mut cmd = Command::new("sudo");
            cmd.arg(script_path);
            cmd
        },
        (None, false) => Command::new(script_path),
    }
}

/// Stream lines from a child process pipe to the shared log in real-time.
/// Each line is flushed immediately so users can `tail -f` the log file.
async fn stream_to_log(
    stream: Option<impl AsyncRead + Unpin>,
    log: &Arc<Mutex<ScriptRunLog>>,
    prefix: &str,
) {
    let Some(stream) = stream else { return };
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if let Ok(mut log) = log.lock() {
            log.write_line(prefix, &line);
        }
    }
}

/// Log process exit diagnostics at debug level.
fn log_process_diagnostics(status: std::process::ExitStatus, pid: Option<u32>) {
    let pid = pid.unwrap_or(0);

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        debug!(
            "Script failed. Diagnostics: pid={pid}, exitstatus={:?}, signal={:?}, core_dumped={}",
            status.code(),
            status.signal(),
            status.core_dumped(),
        );
    }

    #[cfg(not(unix))]
    {
        debug!("Script failed. Diagnostics: pid={pid}, exitstatus={:?}", status.code(),);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::file_ops::ensure_executable;
    use crate::system::process_ops::kill_process_group;
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::{NamedTempFile, TempDir};

    fn sample_log() -> Arc<Mutex<ScriptRunLog>> {
        let file = NamedTempFile::new().unwrap();
        Arc::new(Mutex::new(ScriptRunLog::open(file.path()).unwrap()))
    }

    // --- build_command ---

    #[test]
    fn build_command_no_runas_no_sudo() {
        let cmd = build_command(std::path::Path::new("/test/script.sh"), None, false);
        assert_eq!(cmd.as_std().get_program(), "/test/script.sh");
    }

    #[test]
    fn build_command_with_sudo() {
        let cmd = build_command(std::path::Path::new("/test/script.sh"), None, true);
        assert_eq!(cmd.as_std().get_program(), "sudo");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args, ["/test/script.sh"]);
    }

    #[test]
    fn build_command_with_runas() {
        let cmd = build_command(std::path::Path::new("/test/script.sh"), Some("deploy"), false);
        assert_eq!(cmd.as_std().get_program(), "su");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args, ["deploy", "-c", "/test/script.sh"]);
    }

    #[test]
    fn build_command_with_runas_and_sudo() {
        let cmd = build_command(std::path::Path::new("/test/script.sh"), Some("deploy"), true);
        assert_eq!(cmd.as_std().get_program(), "sudo");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args, ["su", "deploy", "-c", "/test/script.sh"]);
    }

    // --- ensure_executable ---

    #[cfg(unix)]
    #[test]
    fn ensure_executable_adds_execute_bits() {
        use std::os::unix::fs::PermissionsExt;

        let file = NamedTempFile::new().unwrap();
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o644)).unwrap();

        ensure_executable(file.path()).unwrap();

        let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
        assert_ne!(mode & 0o111, 0, "execute bits should be set");
    }

    #[cfg(unix)]
    #[test]
    fn ensure_executable_preserves_existing_bits() {
        use std::os::unix::fs::PermissionsExt;

        let file = NamedTempFile::new().unwrap();
        std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        ensure_executable(file.path()).unwrap();

        let mode = std::fs::metadata(file.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "permissions should be unchanged");
    }

    #[test]
    fn ensure_executable_nonexistent_file() {
        let result = ensure_executable(std::path::Path::new("/nonexistent/script.sh"));
        assert!(result.is_err());
    }

    // --- kill_process_group ---

    #[test]
    fn kill_process_group_none_is_noop() {
        kill_process_group(None);
    }

    #[test]
    fn kill_process_group_invalid_pid_is_noop() {
        kill_process_group(Some(999_999_999));
    }

    // --- Script::new ---

    #[test]
    fn new_captures_fields() {
        let mut env = HashMap::new();
        env.insert("KEY".to_string(), "VAL".to_string());

        let script = Script::new(
            "/test/script.sh".into(),
            Some("deploy".to_string()),
            true,
            &env,
            sample_log(),
        );

        let debug = format!("{script:?}");
        assert!(debug.contains("script.sh"));
        assert!(debug.contains("deploy"));
        assert!(debug.contains("KEY"));
    }

    #[test]
    fn new_no_runas() {
        let script = Script::new(
            "/test/script.sh".into(),
            None,
            false,
            &HashMap::<String, String>::new(),
            sample_log(),
        );

        let debug = format!("{script:?}");
        assert!(debug.contains("runas: None"));
        assert!(debug.contains("sudo: false"));
    }

    // --- Script::execute (error path) ---

    #[test]
    fn execute_nonexistent_script() {
        let script = Script::new(
            "/nonexistent/script.sh".into(),
            None,
            false,
            &HashMap::<String, String>::new(),
            sample_log(),
        );

        let result = script.execute(Duration::from_secs(10));
        assert!(result.is_err());
    }

    // --- Integration: execute with log capture ---

    #[cfg(unix)]
    #[test]
    fn execute_captures_stdout_and_stderr() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("test.sh");
        std::fs::write(&script_path, "#!/bin/sh\necho hello\necho oops >&2\n").unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log_path = dir.path().join("scripts.log");
        let log = Arc::new(Mutex::new(ScriptRunLog::open(&log_path).unwrap()));

        let script = Script::new(
            script_path,
            None,
            false,
            &HashMap::<String, String>::new(),
            Arc::clone(&log),
        );

        let exit_code = script.execute(Duration::from_secs(10)).unwrap();
        assert_eq!(exit_code, 0);

        let entries = log.lock().unwrap().entries();
        let has_stdout = entries.iter().any(|e| e.contains("[stdout]hello"));
        let has_stderr = entries.iter().any(|e| e.contains("[stderr]oops"));
        assert!(has_stdout, "expected stdout entry, got: {entries:?}");
        assert!(has_stderr, "expected stderr entry, got: {entries:?}");
    }

    #[cfg(unix)]
    #[test]
    fn execute_nonzero_exit_logs_diagnostics() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("fail.sh");
        std::fs::write(&script_path, "#!/bin/sh\nexit 42\n").unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log = Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).unwrap()));

        let script = Script::new(
            script_path,
            None,
            false,
            &HashMap::<String, String>::new(),
            Arc::clone(&log),
        );

        let exit_code = script.execute(Duration::from_secs(10)).unwrap();
        assert_eq!(exit_code, 42);
    }
}
