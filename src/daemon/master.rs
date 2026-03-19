//! @risk high
//!
//! Master daemon process: daemonize, spawn workers, monitor, shutdown.

use std::path::Path;
use std::process;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::Duration;

use tracing::{error, info, warn};

use super::pid_file::PidFile;
use super::signal::{self, ShutdownFlag};
use super::{
    DEFAULT_KILL_WAIT_SECS, DEFAULT_PID_DIR, DEFAULT_PID_FILE, WORKER_RESPAWN_DELAY_SECS,
    is_process_alive, send_sigterm,
};
use crate::command_port::{self, AgentState};
use crate::runtime::DeploymentTracker;

/// Timeout for waiting on worker to exit during shutdown (seconds).
///
/// NOTE: If `kill_wait_secs` is configured below this value, systemd may kill
/// the master before the worker wait completes. With the default 7200s this is
/// not an issue. The original agent uses a fixed 5s delay without a bounded worker
/// wait, so this is a safeguard.
const WORKER_SHUTDOWN_TIMEOUT_SECS: u64 = 30;

/// Result of a [`Master::stop`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopOutcome {
    /// A running agent was successfully stopped.
    Stopped,
    /// No agent was running (no PID file or stale PID).
    NotRunning,
}

/// Master daemon configuration.
#[derive(Debug)]
pub struct MasterConfig {
    /// Directory for PID file.
    pub pid_dir: String,
    /// PID filename.
    pub pid_filename: String,
    /// Max wait time for graceful shutdown (seconds).
    pub kill_wait_secs: u64,
    /// Enable the command port.
    pub enable_command_port: bool,
    /// Directory for command port discovery file.
    pub state_dir: String,
}

impl Default for MasterConfig {
    fn default() -> Self {
        Self {
            pid_dir: DEFAULT_PID_DIR.to_string(),
            pid_filename: DEFAULT_PID_FILE.to_string(),
            kill_wait_secs: DEFAULT_KILL_WAIT_SECS,
            enable_command_port: false,
            state_dir: DEFAULT_PID_DIR.to_string(),
        }
    }
}

/// The master daemon process.
#[derive(Debug)]
pub struct Master {
    config: MasterConfig,
    pid_file: PidFile,
    shutdown: ShutdownFlag,
}

impl Master {
    /// Create a new master daemon.
    ///
    /// Each `Master` instance owns its own [`ShutdownFlag`]. Signal handlers
    /// are registered inside [`start()`](Self::start) and bind to this
    /// instance's flag, so the flag is not shared across `Master` instances.
    #[must_use]
    pub fn new(config: MasterConfig) -> Self {
        let pid_file = PidFile::new(Path::new(&config.pid_dir), &config.pid_filename);
        Self { config, pid_file, shutdown: ShutdownFlag::new() }
    }

    /// Start the daemon: write PID, register signals, spawn worker, monitor.
    ///
    /// # Errors
    /// Returns an error if PID file write or signal registration fails.
    ///
    /// TODO: Introduce a `DaemonError` enum (or `StartOutcome`) to distinguish
    /// "already running" from I/O failures without relying on `ErrorKind`.
    /// This mirrors the `StopOutcome` pattern used by [`stop()`](Self::stop).
    pub fn start(&self) -> std::io::Result<()> {
        // Check if already running.
        // NOTE: There is an inherent TOCTOU window between the liveness check and
        // PID file write. File locking (flock) could close this gap but is not
        // required for the current deployment model.
        match self.pid_file.read()? {
            Some(pid) if is_process_alive(pid) => {
                warn!("Agent is already running (pid {pid})");
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!("agent already running (pid {pid})"),
                ));
            },
            // Stale PID (write() will clean up via remove_stale()) or no PID file
            Some(_) | None => {},
        }

        self.pid_file.write()?;
        if let Err(e) = signal::register_shutdown_handlers(&self.shutdown) {
            let _ = self.pid_file.remove();
            return Err(e);
        }

        info!("Master daemon started (pid {})", process::id());

        // Start command port if enabled.
        let state = if self.config.enable_command_port {
            let discovery = Path::new(&self.config.state_dir).join(".command-port");
            match command_port::start(&discovery) {
                Ok((_handle, state)) => Some(state),
                Err(e) => {
                    warn!("Failed to start command port: {e}");
                    None
                },
            }
        } else {
            None
        };

        self.monitor_loop(state.as_ref());
        if let Err(e) = self.pid_file.remove() {
            warn!("Failed to remove PID file on exit: {e}");
        }
        info!("Master daemon exited");
        Ok(())
    }

    /// Stop a running daemon by reading its PID and sending SIGTERM.
    ///
    /// Stop the agent: checks for in-progress deployments before sending signal.
    ///
    /// Returns [`StopOutcome::Stopped`] if a running agent was stopped, or
    /// [`StopOutcome::NotRunning`] if no agent was found.
    ///
    /// # Errors
    /// Returns an error if the PID file can't be read, the signal fails,
    /// a deployment is in progress, or the agent doesn't exit within timeout.
    pub fn stop<T: DeploymentTracker>(&self, tracker: Option<&T>) -> std::io::Result<StopOutcome> {
        let Some(pid) = self.pid_file.read()? else {
            info!("Agent is not running (no PID file)");
            return Ok(StopOutcome::NotRunning);
        };

        if !is_process_alive(pid) {
            info!("Agent is not running (pid {pid} not found), cleaning up PID file");
            self.pid_file.remove()?;
            return Ok(StopOutcome::NotRunning);
        }

        // Refuse to stop if a deployment is in progress
        if let Some(t) = tracker {
            match t.is_deployment_in_progress() {
                Ok(true) => {
                    return Err(std::io::Error::other(
                        "cannot stop: deployment lifecycle event in progress",
                    ));
                },
                Err(e) => {
                    return Err(std::io::Error::other(format!(
                        "cannot determine deployment status: {e}"
                    )));
                },
                Ok(false) => {},
            }
        }

        // NOTE: PID file cleanup happens in the master process itself (end of
        // start()) when it exits the monitor loop after receiving SIGTERM.
        // The CLI `stop` caller does not remove the PID file.
        info!("Sending SIGTERM to agent (pid {pid})");
        send_sigterm(pid)?;

        // Wait for exit up to kill_wait_secs
        let deadline = std::time::Instant::now() + Duration::from_secs(self.config.kill_wait_secs);
        while std::time::Instant::now() < deadline {
            if !is_process_alive(pid) {
                info!("Agent stopped successfully");
                return Ok(StopOutcome::Stopped);
            }
            thread::sleep(Duration::from_millis(500));
        }

        warn!("Agent did not exit within {} seconds", self.config.kill_wait_secs);
        Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "agent did not exit within timeout",
        ))
    }

    /// Report whether the agent is running.
    ///
    /// # Errors
    /// Returns an error if the PID file can't be read.
    pub fn status(&self) -> std::io::Result<bool> {
        Ok(self.pid_file.is_running())
    }

    /// Wait for a worker child process to exit, escalating to SIGKILL after
    /// [`WORKER_SHUTDOWN_TIMEOUT_SECS`].
    fn wait_for_worker_exit(child: &mut process::Child) {
        let deadline =
            std::time::Instant::now() + Duration::from_secs(WORKER_SHUTDOWN_TIMEOUT_SECS);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if std::time::Instant::now() >= deadline => {
                    warn!("Worker did not exit within timeout, sending SIGKILL");
                    let _ = child.kill();
                    let _ = child.wait(); // reap zombie
                    return;
                },
                Ok(None) => thread::sleep(Duration::from_millis(500)),
                Err(e) => {
                    warn!("Failed to check worker status: {e}, assuming exited");
                    return;
                },
            }
        }
    }

    /// Core monitoring loop: spawn worker, respawn on crash, exit on shutdown.
    ///
    /// The inner `try_wait()` loop polls at 500ms intervals rather than blocking
    /// on `child.wait()`. This allows checking the shutdown flag between iterations.
    /// A future optimization could use a pipe or condvar to wake the blocked wait on signal.
    ///
    /// TODO: Add exponential backoff or crash counter to avoid tight respawn
    /// loops when the worker crashes immediately on startup (e.g., config error).
    fn monitor_loop(&self, state: Option<&Arc<RwLock<AgentState>>>) {
        let mut restarts: u32 = 0;
        while !self.shutdown.is_set() {
            match super::worker::spawn() {
                Ok(mut child) => {
                    let pid = child.id();
                    info!("Monitoring worker (pid {pid})");
                    if let Some(s) = state {
                        let mut s = s.write().unwrap_or_else(std::sync::PoisonError::into_inner);
                        s.worker_pid = Some(pid);
                        s.worker_alive = true;
                    }
                    loop {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                if let Some(s) = state {
                                    let mut s = s
                                        .write()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    s.worker_alive = false;
                                }
                                if self.shutdown.is_set() {
                                    info!("Worker exited during shutdown");
                                    return;
                                }
                                restarts += 1;
                                if let Some(s) = state {
                                    let mut s = s
                                        .write()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                                    s.worker_restarts = restarts;
                                }
                                warn!(
                                    "Worker exited with status {status}, respawning in {WORKER_RESPAWN_DELAY_SECS}s"
                                );
                                thread::sleep(Duration::from_secs(WORKER_RESPAWN_DELAY_SECS));
                                break;
                            },
                            Ok(None) => {
                                if self.shutdown.is_set() {
                                    info!("Shutdown requested, stopping worker");
                                    let _ = send_sigterm(child.id());
                                    Self::wait_for_worker_exit(&mut child);
                                    return;
                                }
                                thread::sleep(Duration::from_millis(500));
                            },
                            Err(e) => {
                                error!("Failed to check worker status: {e}");
                                thread::sleep(Duration::from_secs(WORKER_RESPAWN_DELAY_SECS));
                                break;
                            },
                        }
                    }
                },
                Err(e) => {
                    error!("Failed to spawn worker: {e}");
                    if let Some(s) = state {
                        let mut s = s.write().unwrap_or_else(std::sync::PoisonError::into_inner);
                        s.worker_alive = false;
                        s.worker_pid = None;
                    }
                    if self.shutdown.is_set() {
                        return;
                    }
                    thread::sleep(Duration::from_secs(WORKER_RESPAWN_DELAY_SECS));
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    type TestTracker =
        crate::runtime::FileBasedDeploymentTracker<crate::system::SystemFileOperations>;

    fn test_master(dir: &TempDir) -> Master {
        let config = MasterConfig {
            pid_dir: dir.path().to_string_lossy().to_string(),
            pid_filename: "test.pid".to_string(),
            kill_wait_secs: 2,
            enable_command_port: false,
            state_dir: dir.path().to_string_lossy().to_string(),
        };
        Master::new(config)
    }

    #[test]
    fn default_config_values() {
        let config = MasterConfig::default();
        assert_eq!(config.pid_dir, DEFAULT_PID_DIR);
        assert_eq!(config.pid_filename, DEFAULT_PID_FILE);
        assert_eq!(config.kill_wait_secs, DEFAULT_KILL_WAIT_SECS);
    }

    #[test]
    fn status_false_when_not_running() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        assert!(!master.status().unwrap());
    }

    #[test]
    fn status_true_when_pid_file_has_current_pid() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        master.pid_file.write().unwrap();
        assert!(master.status().unwrap());
    }

    #[test]
    fn stop_returns_not_running_when_no_pid_file() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        assert_eq!(master.stop::<TestTracker>(None).unwrap(), StopOutcome::NotRunning);
    }

    #[test]
    fn stop_cleans_stale_pid_and_returns_not_running() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        std::fs::write(master.pid_file.path(), "99999999").unwrap();
        assert_eq!(master.stop::<TestTracker>(None).unwrap(), StopOutcome::NotRunning);
        assert!(!master.pid_file.path().exists());
    }

    #[test]
    fn start_fails_if_already_running() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        master.pid_file.write().unwrap();
        let err = master.start().unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn stop_refuses_during_deployment() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        // PID file contains current test process PID, so the liveness check passes
        // and stop() proceeds to the deployment check.
        master.pid_file.write().unwrap();

        let tracker_dir = dir.path().join("tracker");
        let tracker = TestTracker::new(tracker_dir);
        tracker.start_tracking("d-123", "cmd-456").unwrap();

        let err = master.stop(Some(&tracker)).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::Other);
        assert!(err.to_string().contains("deployment lifecycle event in progress"));
    }

    #[test]
    fn stop_fails_when_tracker_errors() {
        use crate::runtime::deployment_tracker::{ActiveDeployment, DeploymentTrackerError};

        struct ErrorTracker;
        impl DeploymentTracker for ErrorTracker {
            fn start_tracking(&self, _: &str, _: &str) -> Result<(), DeploymentTrackerError> {
                Ok(())
            }
            fn stop_tracking(&self, _: &str) -> Result<(), DeploymentTrackerError> {
                Ok(())
            }
            fn get_active_deployment(
                &self,
            ) -> Result<Option<ActiveDeployment>, DeploymentTrackerError> {
                Ok(None)
            }
            fn is_deployment_in_progress(&self) -> Result<bool, DeploymentTrackerError> {
                Err(DeploymentTrackerError::Io(std::io::Error::other("disk error")))
            }
            fn clean_all(&self) -> Result<(), DeploymentTrackerError> {
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        // PID file contains current test process PID, so the liveness check passes.
        master.pid_file.write().unwrap();

        let err = master.stop(Some(&ErrorTracker)).unwrap_err();
        assert!(err.to_string().contains("cannot determine deployment status"));
    }

    #[test]
    fn start_propagates_pid_file_read_error() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        // Write invalid content so read() returns Err (not Ok(None))
        std::fs::write(master.pid_file.path(), "not-a-number").unwrap();
        let err = master.start().unwrap_err();
        assert!(err.to_string().contains("invalid PID"), "expected parse error, got: {err}");
    }
}
