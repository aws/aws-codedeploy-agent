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
use crate::system::process_ops::{force_kill_process_group, kill_process_group};
use std::collections::HashMap;
use std::hash::BuildHasher;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncRead, BufReader};
use tokio::process::Command;
use tracing::{debug, info};

/// Grace period between SIGTERM and the SIGKILL escalation on timeout. A script
/// that ignores SIGTERM gets this long to exit before being force-killed.
const SIGKILL_GRACE: Duration = Duration::from_secs(5);

/// Opt-in hook environment hardening. All flags default to `false`, preserving
/// backwards-compatible hook behavior (full env inheritance, no PowerShell
/// profile suppression).
#[derive(Debug, Clone, Copy, Default)]
pub struct HookEnvPolicy {
    /// Strip `LD_PRELOAD`/`LD_LIBRARY_PATH`/`LD_AUDIT` from the hook env.
    pub strip_loader_vars: bool,
    /// Clear the inherited env and rebuild from shell basics + deployment vars.
    pub restrict_to_allowlist: bool,
    /// Run `.ps1` hooks with `-NoProfile -NonInteractive` (Windows). The
    /// Windows analog of `strip_loader_vars`: profile scripts inject code
    /// into the hook's startup exactly like `LD_PRELOAD` does on Unix.
    pub disable_powershell_profile: bool,
}

/// A deployment lifecycle script to be executed.
#[derive(Debug)]
pub struct Script {
    path: PathBuf,
    runas: Option<String>,
    sudo: bool,
    env_vars: HashMap<String, String>,
    env_policy: HookEnvPolicy,
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
        Self::with_env_policy(path, runas, sudo, env_vars, HookEnvPolicy::default(), log)
    }

    /// Like [`Script::new`] but with an explicit [`HookEnvPolicy`]. `new`
    /// defaults the policy to all-`false` (full env inheritance).
    #[must_use]
    pub fn with_env_policy<S: BuildHasher>(
        path: PathBuf,
        runas: Option<String>,
        sudo: bool,
        env_vars: &HashMap<String, String, S>,
        env_policy: HookEnvPolicy,
        log: Arc<Mutex<ScriptRunLog>>,
    ) -> Self {
        Self {
            path,
            runas,
            sudo,
            env_vars: env_vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            env_policy,
            log,
        }
    }

    /// Whether this script will be executed under `sudo`.
    ///
    /// Test-only accessor used to verify appspec → `Script` wiring without
    /// running the script.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn sudo(&self) -> bool {
        self.sudo
    }

    /// The `runas` user, if any.
    ///
    /// Test-only accessor used to verify appspec → `Script` wiring without
    /// running the script.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn runas(&self) -> Option<&str> {
        self.runas.as_deref()
    }

    /// Sync entry point. Spawns the script, streams output, waits up to `timeout`.
    ///
    /// Caller is responsible for pre-flight checks (existence, executable permission).
    ///
    /// # Errors
    /// Returns an error if the script fails to spawn, exceeds the timeout,
    /// or the tokio runtime cannot be created.
    pub fn execute(self, timeout: Duration) -> Result<i32, String> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;

        rt.block_on(self.execute_async(timeout))
    }

    /// Apply the hook environment policy to `cmd`: the deployment vars on top
    /// of the (optionally allowlist-restricted) inherited env, with the AWS
    /// credential-override vars always removed and `LD_*` stripped under the
    /// opt-in flag.
    fn apply_env_policy(&self, cmd: &mut Command) {
        // Default: inherit the agent's full env, with the deployment vars layered
        // on via `cmd.envs`. `restrict_to_allowlist` opts into clearing it first.
        if self.env_policy.restrict_to_allowlist {
            cmd.env_clear();
            // Shell basics required to run the hook and (on Windows) the interpreter.
            #[cfg(unix)]
            let basics: &[&str] = &["PATH", "HOME", "USER"];
            #[cfg(windows)]
            let basics: &[&str] = &[
                "PATH",
                "SystemRoot",
                "COMSPEC",
                "PATHEXT",
                "TEMP",
                "TMP",
                "windir",
            ];
            for basic in basics {
                if let Ok(val) = std::env::var(basic) {
                    cmd.env(basic, val);
                }
            }
        }

        cmd.envs(&self.env_vars);

        // Never expose AWS credential-override vars to hooks. Ordered after
        // `envs()` so an AppSpec `environment:` entry cannot re-introduce them.
        for var in [
            "AWS_ACCESS_KEY_ID",
            "AWS_SECRET_ACCESS_KEY",
            "AWS_CREDENTIAL_FILE",
        ] {
            cmd.env_remove(var);
        }

        // Strip loader vars last, so an AppSpec `environment:` entry cannot re-add them.
        #[cfg(unix)]
        if self.env_policy.strip_loader_vars {
            for var in ["LD_PRELOAD", "LD_LIBRARY_PATH", "LD_AUDIT"] {
                cmd.env_remove(var);
            }
        }
    }

    /// Async implementation. Spawns the script, streams stdout/stderr to the
    /// shared log in real-time, and waits with a shared deadline.
    ///
    /// Uses `DeadlineJoiner` to share one deadline across three phases:
    /// 1. Wait for process exit (up to deadline) — timeout → kill + `"timeout"`
    /// 2. Wait for stdout to close (remaining time) — timeout → `"outputs_left_open"`
    /// 3. Wait for stderr to close (remaining time) — timeout → `"outputs_left_open"`
    ///
    /// # Errors
    /// Returns `"timeout"` if the process exceeds the deadline, or
    /// `"outputs_left_open"` if the process exits but stdout/stderr remain open.
    pub async fn execute_async(self, timeout: Duration) -> Result<i32, String> {
        // Audit log: every script execution records the user context switch
        // (source user → target user) before the spawn so we get a record
        // even if the spawn itself fails.
        #[cfg(unix)]
        let source_user = nix::unistd::User::from_uid(nix::unistd::geteuid())
            .ok()
            .flatten()
            .map_or_else(|| nix::unistd::geteuid().to_string(), |u| u.name);
        #[cfg(windows)]
        let source_user = std::env::var("USERNAME").unwrap_or_else(|_| "(unknown)".to_string());

        info!(
            source_user = %source_user,
            target_user = self.runas.as_deref().unwrap_or("(self)"),
            sudo = self.sudo,
            script = %self.path.display(),
            "Lifecycle script user context switch"
        );

        let mut cmd = build_command(
            &self.path,
            self.runas.as_deref(),
            self.sudo,
            self.env_policy.disable_powershell_profile,
        );

        self.apply_env_policy(&mut cmd);
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
            cmd.creation_flags(CREATE_NEW_PROCESS_GROUP);
        }

        let mut child = cmd.spawn().map_err(|e| e.to_string())?;
        let pid = child.id();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let joiner = super::deadline_joiner::DeadlineJoiner::new(timeout);

        // Phase 1: wait for process exit.
        let stdout_log = Arc::clone(&self.log);
        let stderr_log = Arc::clone(&self.log);
        let stdout_handle =
            tokio::spawn(async move { stream_to_log(stdout, &stdout_log, "[stdout]").await });
        let stderr_handle =
            tokio::spawn(async move { stream_to_log(stderr, &stderr_log, "[stderr]").await });

        let status = match joiner.join(child.wait()).await {
            Ok(Ok(status)) => status,
            Ok(Err(e)) => return Err(e.to_string()), // GRCOV_IGNORE_LINE
            Err(()) => {
                // GRCOV_IGNORE_START
                // Timeout: SIGTERM the group, then SIGKILL (uncatchable) if it
                // outlasts the grace period so a SIGTERM-ignoring hook is still
                // reaped. The child is unreaped here, so its PID can't be reused
                // before the wait() below — SIGKILL can't hit an unrelated
                // process.
                //
                // Every wait after the signal is bounded: a hook that traps
                // SIGTERM must not be able to block the agent indefinitely, so
                // neither the grace-period wait nor the post-SIGKILL reap is
                // allowed to run without a deadline.
                kill_process_group(pid);
                if tokio::time::timeout(SIGKILL_GRACE, child.wait()).await.is_err() {
                    force_kill_process_group(pid);
                    // Best-effort reap, but keep it bounded: SIGKILL is
                    // uncatchable yet not instantaneous — a process in
                    // uninterruptible sleep (D-state, e.g. stuck on NFS/FUSE
                    // I/O) is not reaped until the I/O completes. An unbounded
                    // wait here would reintroduce the hang this escalation
                    // exists to prevent, so we abandon the wait after the grace
                    // period and let tokio's orphan reaper collect the child on
                    // SIGCHLD.
                    let _ = tokio::time::timeout(SIGKILL_GRACE, child.wait()).await;
                }
                return Err("timeout".to_string());
                // GRCOV_IGNORE_END
            },
        };

        // Phase 2: wait for stdout to close.
        // GRCOV_STOP_COVERAGE
        if joiner.join(stdout_handle).await.is_err() {
            return Err("outputs_left_open".to_string());
        }

        // Phase 3: wait for stderr to close.
        if joiner.join(stderr_handle).await.is_err() {
            return Err("outputs_left_open".to_string());
        }
        // GRCOV_BEGIN_COVERAGE

        let code = status.code().unwrap_or(1);
        if code != 0 {
            log_process_diagnostics(status, pid);
        }

        Ok(code)
    }
}

/// Build the command for script execution (Unix).
///
/// | `runas` | `sudo` | Command                              |
/// |---------|--------|--------------------------------------|
/// | Some    | true   | `sudo su <user> -c <script>`         |
/// | Some    | false  | `su <user> -c <script>`              |
/// | None    | true   | `sudo <script>`                      |
/// | None    | false  | `<script>` (run as agent user)       |
#[cfg(unix)]
pub(super) fn build_command(
    script_path: &Path,
    runas: Option<&str>,
    sudo: bool,
    _disable_powershell_profile: bool,
) -> Command {
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

/// Build the command for script execution (Windows).
///
/// `runas`/`sudo` are not supported on Windows. The AppSpec validator rejects
/// `permissions` on Windows upfront; this warning is defense-in-depth.
///
/// Build the command for `.ps1` / other-extension scripts on Windows.
///
/// Default (`disable_powershell_profile = false`): only `-ExecutionPolicy
/// Bypass -File`, the backwards-compatible invocation.
///
/// Opt-in hardening (`disable_powershell_profile = true`): adds `-NoProfile
/// -NonInteractive` before `-ExecutionPolicy Bypass -File`, preventing
/// profile scripts from injecting code and suppressing interactive prompts
/// that would hang a service-context agent.
///
/// Other scripts (`.bat`, `.cmd`, `.exe`) run directly under both policies.
#[cfg(windows)]
pub(super) fn build_command(
    script_path: &Path,
    runas: Option<&str>,
    sudo: bool,
    disable_powershell_profile: bool,
) -> Command {
    if runas.is_some() || sudo {
        tracing::warn!("runas/sudo not supported on Windows; ignoring");
    }

    let is_ps1 = script_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ps1"));

    if is_ps1 {
        let mut cmd = Command::new("powershell.exe");
        if disable_powershell_profile {
            cmd.args(["-NoProfile", "-NonInteractive"]);
        }
        cmd.args(["-ExecutionPolicy", "Bypass", "-File"]);
        cmd.arg(script_path);
        cmd
    } else {
        Command::new(script_path)
    }
}

/// Max bytes buffered from one output line before force-flushing as a chunk.
/// `scripts.log` rotation is checked per `write_line`, so chunking long lines
/// keeps a newline-free blob from defeating the 64 MiB rotation cap and bounds
/// memory.
const MAX_LOG_LINE_BYTES: usize = 64 * 1024;

/// Stream a child process pipe to the shared log in real-time. Reads bytes (not
/// `.lines()`) so a newline-free line over [`MAX_LOG_LINE_BYTES`] is emitted in
/// bounded chunks rather than accumulated without limit.
async fn stream_to_log(
    stream: Option<impl AsyncRead + Unpin>,
    log: &Arc<Mutex<ScriptRunLog>>,
    prefix: &str,
) {
    use tokio::io::AsyncReadExt;
    let Some(stream) = stream else { return };
    let mut reader = BufReader::new(stream);
    let mut buf: Vec<u8> = Vec::with_capacity(MAX_LOG_LINE_BYTES);
    let mut byte = [0u8; 1];

    let flush = |log: &Arc<Mutex<ScriptRunLog>>, buf: &mut Vec<u8>| {
        if buf.is_empty() {
            return;
        }
        let line = String::from_utf8_lossy(buf);
        if let Ok(mut log) = log.lock() {
            log.write_line(prefix, &line);
        }
        buf.clear();
    };

    loop {
        match reader.read(&mut byte).await {
            // EOF (Ok(0)) or a read error: stop draining this stream.
            Ok(0) | Err(_) => break,
            Ok(_) => {
                if byte[0] == b'\n' {
                    flush(log, &mut buf);
                } else {
                    buf.push(byte[0]);
                    if buf.len() >= MAX_LOG_LINE_BYTES {
                        // Newline-free run hit the chunk cap — flush so rotation
                        // is evaluated and memory stays bounded.
                        flush(log, &mut buf);
                    }
                }
            },
        }
    }
    flush(log, &mut buf); // trailing partial line (no final newline)
}

/// Log process exit diagnostics at debug level.
fn log_process_diagnostics(status: std::process::ExitStatus, pid: Option<u32>) {
    let pid = pid.unwrap_or(0);

    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        // GRCOV_STOP_COVERAGE
        debug!(
            "Script failed. Diagnostics: pid={pid}, exitstatus={:?}, signal={:?}, core_dumped={}",
            status.code(),
            status.signal(),
            status.core_dumped(),
        );
        // GRCOV_BEGIN_COVERAGE
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

    #[tokio::test]
    async fn stream_to_log_chunks_long_newline_free_output() {
        // A hook that emits a huge blob with no newline must be split into
        // bounded chunks so scripts.log rotation (per write_line) can fire and
        // memory stays bounded — instead of one unbounded write that defeats the
        // 64 MiB rotation cap. Asserted on the FILE (the in-memory diagnostics
        // buffer is byte-capped and only keeps the tail, so it can't show the
        // chunk count).
        let file = NamedTempFile::new().unwrap();
        let path = file.path().to_path_buf();
        let log = Arc::new(Mutex::new(ScriptRunLog::open(&path).unwrap()));
        let blob = vec![b'A'; MAX_LOG_LINE_BYTES * 3 + 100]; // ~192 KiB, no '\n'
        stream_to_log(Some(std::io::Cursor::new(blob)), &log, "[stdout]").await;

        // Each write_line emits one timestamped, newline-terminated record.
        // Chunking the blob into <=64 KiB pieces yields >=4 records.
        let contents = std::fs::read_to_string(&path).unwrap();
        let records = contents.lines().filter(|l| l.contains("[stdout]")).count();
        assert!(
            records >= 4,
            "expected newline-free blob chunked into >=4 records, got {records}"
        );
        // No single record exceeds the chunk bound (plus timestamp/prefix slack).
        for line in contents.lines() {
            assert!(
                line.len() <= MAX_LOG_LINE_BYTES + 64,
                "a log record exceeded the chunk bound: {} bytes",
                line.len()
            );
        }
    }

    #[tokio::test]
    async fn stream_to_log_preserves_normal_lines() {
        let log = sample_log();
        let input = b"first line\nsecond line\n".to_vec();
        stream_to_log(Some(std::io::Cursor::new(input)), &log, "[stdout]").await;
        let entries = log.lock().unwrap().entries();
        assert_eq!(entries.len(), 2);
        assert!(entries[0].contains("first line"));
        assert!(entries[1].contains("second line"));
    }

    // --- build_command ---

    #[cfg(unix)]
    #[test]
    fn build_command_no_runas_no_sudo() {
        let cmd = build_command(std::path::Path::new("/test/script.sh"), None, false, false);
        assert_eq!(cmd.as_std().get_program(), "/test/script.sh");
    }

    #[cfg(unix)]
    #[test]
    fn build_command_with_sudo() {
        let cmd = build_command(std::path::Path::new("/test/script.sh"), None, true, false);
        assert_eq!(cmd.as_std().get_program(), "sudo");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args, ["/test/script.sh"]);
    }

    #[cfg(unix)]
    #[test]
    fn build_command_with_runas() {
        let cmd =
            build_command(std::path::Path::new("/test/script.sh"), Some("deploy"), false, false);
        assert_eq!(cmd.as_std().get_program(), "su");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args, ["deploy", "-c", "/test/script.sh"]);
    }

    #[cfg(unix)]
    #[test]
    fn build_command_with_runas_and_sudo() {
        let cmd =
            build_command(std::path::Path::new("/test/script.sh"), Some("deploy"), true, false);
        assert_eq!(cmd.as_std().get_program(), "sudo");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args, ["su", "deploy", "-c", "/test/script.sh"]);
    }

    #[cfg(windows)]
    #[test]
    fn build_command_ps1_default_no_noprofile() {
        let cmd = build_command(std::path::Path::new(r"C:\scripts\hook.ps1"), None, false, false);
        assert_eq!(cmd.as_std().get_program(), "powershell.exe");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args[0], "-ExecutionPolicy");
        assert_eq!(args[1], "Bypass");
        assert_eq!(args[2], "-File");
        assert_eq!(args[3], r"C:\scripts\hook.ps1");
    }

    #[cfg(windows)]
    #[test]
    fn build_command_ps1_hardened_adds_noprofile() {
        let cmd = build_command(std::path::Path::new(r"C:\scripts\hook.ps1"), None, false, true);
        assert_eq!(cmd.as_std().get_program(), "powershell.exe");
        let args: Vec<_> = cmd.as_std().get_args().collect();
        assert_eq!(args[0], "-NoProfile");
        assert_eq!(args[1], "-NonInteractive");
        assert_eq!(args[2], "-ExecutionPolicy");
        assert_eq!(args[3], "Bypass");
        assert_eq!(args[4], "-File");
        assert_eq!(args[5], r"C:\scripts\hook.ps1");
    }

    #[cfg(windows)]
    #[test]
    fn build_command_bat_runs_directly() {
        let cmd = build_command(std::path::Path::new(r"C:\scripts\hook.bat"), None, false, false);
        assert_eq!(cmd.as_std().get_program(), r"C:\scripts\hook.bat");
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

    /// Default policy: the hook inherits the agent's full environment. A
    /// non-deployment agent var (`CARGO`, always set during `cargo test`) MUST
    /// reach the hook when the default `HookEnvPolicy` (all-`false`) is used.
    #[cfg(unix)]
    #[test]
    fn execute_inherits_agent_env_into_hook_by_default() {
        use std::os::unix::fs::PermissionsExt;

        assert!(std::env::var("CARGO").is_ok(), "precondition: CARGO set in test env");

        let dir = TempDir::new().unwrap();
        let out = dir.path().join("seen.txt");
        let script_path = dir.path().join("inherit.sh");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\nprintf '%s' \"$CARGO\" > '{}'\n", out.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log = Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).unwrap()));
        let script = Script::new(
            script_path,
            None,
            false,
            &HashMap::<String, String>::new(),
            Arc::clone(&log),
        );
        let code = script.execute(Duration::from_secs(10)).unwrap();

        assert_eq!(code, 0);
        let seen = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            seen,
            std::env::var("CARGO").unwrap(),
            "default policy must inherit the agent env, but CARGO did not reach hook"
        );
    }

    /// OPT-IN `restrict_to_allowlist`: when enabled, the hook must NOT inherit
    /// arbitrary agent vars — only PATH/HOME/USER basics + deployment vars. A
    /// non-basic agent var (`CARGO`) must be scrubbed.
    #[cfg(unix)]
    #[test]
    fn execute_restrict_allowlist_does_not_leak_agent_env() {
        use std::os::unix::fs::PermissionsExt;

        assert!(std::env::var("CARGO").is_ok(), "precondition: CARGO set in test env");

        let dir = TempDir::new().unwrap();
        let out = dir.path().join("seen.txt");
        let script_path = dir.path().join("leak.sh");
        std::fs::write(
            &script_path,
            format!("#!/bin/sh\nprintf '%s' \"$CARGO\" > '{}'\n", out.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log = Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).unwrap()));
        let script = Script::with_env_policy(
            script_path,
            None,
            false,
            &HashMap::<String, String>::new(),
            HookEnvPolicy {
                strip_loader_vars: false,
                restrict_to_allowlist: true,
                disable_powershell_profile: false,
            },
            Arc::clone(&log),
        );
        let code = script.execute(Duration::from_secs(10)).unwrap();

        assert_eq!(code, 0);
        let seen = std::fs::read_to_string(&out).unwrap();
        assert_eq!(
            seen, "",
            "restrict_to_allowlist must scrub non-allowlisted agent var: {seen:?}"
        );
    }

    /// The three AWS credential-override names must never reach the hook, even
    /// under the default full-inheritance policy.
    #[cfg(unix)]
    #[test]
    fn execute_scrubs_aws_credential_override_vars() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let out = dir.path().join("creds.txt");
        let script_path = dir.path().join("creds.sh");
        std::fs::write(
            &script_path,
            format!(
                "#!/bin/sh\n\
                 printf 'ID=%s\\n' \"${{AWS_ACCESS_KEY_ID:-UNSET}}\" > '{out}'\n\
                 printf 'SECRET=%s\\n' \"${{AWS_SECRET_ACCESS_KEY:-UNSET}}\" >> '{out}'\n\
                 printf 'FILE=%s\\n' \"${{AWS_CREDENTIAL_FILE:-UNSET}}\" >> '{out}'\n",
                out = out.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Supply the three names via AppSpec env_vars — they must be scrubbed
        // AFTER envs() so a bundle cannot smuggle them in.
        let mut env_vars = HashMap::<String, String>::new();
        env_vars.insert("AWS_ACCESS_KEY_ID".into(), "AKIAEVIL".into());
        env_vars.insert("AWS_SECRET_ACCESS_KEY".into(), "secret".into());
        env_vars.insert("AWS_CREDENTIAL_FILE".into(), "/tmp/creds".into());

        let log = Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).unwrap()));
        let script = Script::new(script_path, None, false, &env_vars, Arc::clone(&log));
        let code = script.execute(Duration::from_secs(10)).unwrap();

        assert_eq!(code, 0);
        let seen = std::fs::read_to_string(&out).unwrap();
        assert!(seen.contains("ID=UNSET"), "AWS_ACCESS_KEY_ID must be scrubbed: {seen}");
        assert!(seen.contains("SECRET=UNSET"), "AWS_SECRET_ACCESS_KEY must be scrubbed: {seen}");
        assert!(seen.contains("FILE=UNSET"), "AWS_CREDENTIAL_FILE must be scrubbed: {seen}");
    }

    /// A hook that traps/ignores SIGTERM and exceeds its timeout must still be
    /// reaped via the SIGKILL escalation. The script writes a marker,
    /// ignores TERM, then sleeps; after the timeout the agent must SIGKILL it so
    /// `execute` returns the "timeout" error rather than hanging.
    #[cfg(unix)]
    #[test]
    fn execute_escalates_to_sigkill_when_sigterm_ignored() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let script_path = dir.path().join("trapper.sh");
        std::fs::write(&script_path, "#!/bin/sh\ntrap '' TERM\nsleep 600\n").unwrap();
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let log = Arc::new(Mutex::new(ScriptRunLog::open(&dir.path().join("log")).unwrap()));
        let script = Script::new(
            script_path,
            None,
            false,
            &HashMap::<String, String>::new(),
            Arc::clone(&log),
        );

        // 1s timeout + 5s SIGKILL grace → resolves in well under the 600s sleep.
        let start = std::time::Instant::now();
        let result = script.execute(Duration::from_secs(1));
        assert_eq!(result, Err("timeout".to_string()));
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "SIGKILL escalation did not reap the SIGTERM-ignoring script in time"
        );
    }
}
