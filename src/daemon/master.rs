//! Master daemon process: daemonize, spawn workers, monitor, shutdown.

use std::path::Path;
use std::process;
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{error, info, warn};

use super::pid_file::PidFile;
use super::signal::{self, ShutdownFlag};
use super::{
    DEFAULT_KILL_WAIT_SECS, DEFAULT_PID_FILE, WORKER_HEALTHY_UPTIME_SECS,
    WORKER_RESPAWN_DELAY_SECS, WORKER_RESPAWN_MAX_DELAY_SECS, default_pid_dir, is_process_alive,
    send_sigterm,
};
use crate::command_port::{self, AgentState};
use crate::runtime::DeploymentTracker;

/// Tracks consecutive rapid worker crashes and computes the next respawn delay.
///
/// Uses exponential backoff (5s, 10s, 20s, 40s, capped at
/// `WORKER_RESPAWN_MAX_DELAY_SECS`) to avoid tight fork loops when the worker crashes
/// immediately on startup (e.g., bad config). A worker that stays alive at least
/// `WORKER_HEALTHY_UPTIME_SECS` resets the counter so transient crashes don't
/// accumulate.
#[derive(Debug)]
struct RespawnBackoff {
    consecutive_rapid_crashes: u32,
}

impl RespawnBackoff {
    const fn new() -> Self {
        Self { consecutive_rapid_crashes: 0 }
    }

    /// Record that a worker exited after running for `uptime` and return the
    /// delay to wait before respawning.
    fn record_crash(&mut self, uptime: Duration) -> Duration {
        if uptime >= Duration::from_secs(WORKER_HEALTHY_UPTIME_SECS) {
            self.consecutive_rapid_crashes = 0;
        } else {
            self.consecutive_rapid_crashes = self.consecutive_rapid_crashes.saturating_add(1);
        }
        self.delay()
    }

    fn delay(&self) -> Duration {
        let exponent = self.consecutive_rapid_crashes.saturating_sub(1).min(32);
        let secs = WORKER_RESPAWN_DELAY_SECS
            .saturating_mul(1_u64 << exponent)
            .min(WORKER_RESPAWN_MAX_DELAY_SECS);
        Duration::from_secs(secs)
    }
}

/// Result of a [`Master::stop`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum StopOutcome {
    /// A running agent was successfully stopped.
    Stopped,
    /// No agent was running (no PID file or stale PID).
    NotRunning,
}

/// Result of a [`Master::start`] call.
///
/// Distinguishes "already running" from I/O failures without relying on
/// [`std::io::ErrorKind::AlreadyExists`], mirroring [`StopOutcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum StartOutcome {
    /// The daemon started, ran its monitor loop, and exited cleanly.
    Started,
    /// An agent was already running; the existing PID is reported.
    AlreadyRunning {
        /// PID of the agent that was already running.
        pid: u32,
    },
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
    /// Mode policy for the PID dir/file, from `restrict_agent_dir_permissions`.
    pub restrict_agent_dir_permissions: bool,
}

impl Default for MasterConfig {
    fn default() -> Self {
        let pid_dir = default_pid_dir().to_string_lossy().into_owned();
        Self {
            pid_dir: pid_dir.clone(),
            pid_filename: DEFAULT_PID_FILE.to_string(),
            kill_wait_secs: DEFAULT_KILL_WAIT_SECS,
            enable_command_port: false,
            state_dir: pid_dir,
            restrict_agent_dir_permissions: false,
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
        let pid_file = PidFile::with_policy(
            Path::new(&config.pid_dir),
            &config.pid_filename,
            config.restrict_agent_dir_permissions,
        );
        Self { config, pid_file, shutdown: ShutdownFlag::new() }
    }

    /// Start the daemon: write PID, register signals, spawn worker, monitor.
    ///
    /// Returns [`StartOutcome::Started`] after the monitor loop exits cleanly,
    /// or [`StartOutcome::AlreadyRunning`] if a live agent already holds the
    /// PID file.
    ///
    /// # Errors
    /// Returns an error if PID file read/write or signal registration fails.
    pub fn start(&self) -> std::io::Result<StartOutcome> {
        // Check if already running.
        // NOTE: There is an inherent TOCTOU window between the liveness check and
        // PID file write. File locking (flock) could close this gap but is not
        // required for the current deployment model.
        if let Some(pid) = self.pid_file.running_pid()? {
            warn!("Agent is already running (pid {pid})");
            return Ok(StartOutcome::AlreadyRunning { pid });
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
        Ok(StartOutcome::Started)
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

    /// Wait up to `timeout_secs` for a worker child process to exit, escalating
    /// to SIGKILL afterward.
    ///
    /// `timeout_secs` is the operator-configured `kill_agent_max_wait_time_seconds`,
    /// giving a worker draining a long deployment on shutdown the configured grace
    /// before SIGKILL.
    fn wait_for_worker_exit(child: &mut process::Child, timeout_secs: u64) {
        let deadline = std::time::Instant::now() + Duration::from_secs(timeout_secs);
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
    /// Respawn uses exponential backoff ([`RespawnBackoff`]) so a worker that crashes
    /// immediately on startup (e.g., bad config) does not trigger a tight fork loop.
    fn monitor_loop(&self, state: Option<&Arc<RwLock<AgentState>>>) {
        let mut restarts: u32 = 0;
        let mut backoff = RespawnBackoff::new();
        while !self.shutdown.is_set() {
            match super::worker::spawn() {
                Ok(mut child) => {
                    let pid = child.id();
                    let started = Instant::now();
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
                                let delay = backoff.record_crash(started.elapsed());
                                warn!(
                                    "Worker exited with status {status}, respawning in {}s",
                                    delay.as_secs()
                                );
                                self.interruptible_sleep(delay);
                                break;
                            },
                            Ok(None) => {
                                if self.shutdown.is_set() {
                                    info!("Shutdown requested, stopping worker");
                                    let _ = send_sigterm(child.id());
                                    Self::wait_for_worker_exit(
                                        &mut child,
                                        self.config.kill_wait_secs,
                                    );
                                    return;
                                }
                                thread::sleep(Duration::from_millis(500));
                            },
                            Err(e) => {
                                error!("Failed to check worker status: {e}");
                                let delay = backoff.record_crash(started.elapsed());
                                self.interruptible_sleep(delay);
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
                    let delay = backoff.record_crash(Duration::ZERO);
                    self.interruptible_sleep(delay);
                },
            }
        }
    }

    /// Sleep in 500ms slices so shutdown is detected promptly even with a long backoff.
    fn interruptible_sleep(&self, total: Duration) {
        let slice = Duration::from_millis(500);
        let deadline = Instant::now() + total;
        while Instant::now() < deadline {
            if self.shutdown.is_set() {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            thread::sleep(remaining.min(slice));
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
            restrict_agent_dir_permissions: false,
        };
        Master::new(config)
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn default_config_values() {
        let config = MasterConfig::default();
        assert_eq!(config.pid_dir, "/opt/codedeploy-agent/state/.pid");
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
    fn start_returns_already_running_when_pid_file_live() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        master.pid_file.write().unwrap();
        let outcome = master.start().unwrap();
        assert_eq!(outcome, StartOutcome::AlreadyRunning { pid: process::id() });
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

    #[test]
    fn respawn_backoff_starts_at_base_delay() {
        let mut b = RespawnBackoff::new();
        let d = b.record_crash(Duration::ZERO);
        assert_eq!(d, Duration::from_secs(WORKER_RESPAWN_DELAY_SECS));
    }

    #[test]
    fn respawn_backoff_doubles_on_rapid_crashes() {
        let mut b = RespawnBackoff::new();
        let d1 = b.record_crash(Duration::ZERO);
        let d2 = b.record_crash(Duration::ZERO);
        let d3 = b.record_crash(Duration::ZERO);
        assert_eq!(d1, Duration::from_secs(5));
        assert_eq!(d2, Duration::from_secs(10));
        assert_eq!(d3, Duration::from_secs(20));
    }

    #[test]
    fn respawn_backoff_caps_at_max_delay() {
        let mut b = RespawnBackoff::new();
        // Feed many rapid crashes; the delay must never exceed the cap.
        let mut last = Duration::ZERO;
        for _ in 0..20 {
            last = b.record_crash(Duration::ZERO);
        }
        assert_eq!(last, Duration::from_secs(WORKER_RESPAWN_MAX_DELAY_SECS));
    }

    #[test]
    fn respawn_backoff_resets_after_healthy_uptime() {
        let mut b = RespawnBackoff::new();
        b.record_crash(Duration::ZERO);
        b.record_crash(Duration::ZERO);
        // Worker ran long enough to be healthy; counter resets, so this crash
        // is treated as the first again.
        let d = b.record_crash(Duration::from_secs(WORKER_HEALTHY_UPTIME_SECS));
        assert_eq!(d, Duration::from_secs(WORKER_RESPAWN_DELAY_SECS));
        // Next rapid crash starts over; must not inherit the pre-reset counter.
        let d_next = b.record_crash(Duration::ZERO);
        assert_eq!(d_next, Duration::from_secs(WORKER_RESPAWN_DELAY_SECS));
        // And the one after that begins doubling from the base.
        let d_after = b.record_crash(Duration::ZERO);
        assert_eq!(d_after, Duration::from_secs(WORKER_RESPAWN_DELAY_SECS * 2));
    }

    #[test]
    fn interruptible_sleep_returns_promptly_on_shutdown() {
        let dir = TempDir::new().unwrap();
        let master = test_master(&dir);
        master.shutdown.set();
        let start = Instant::now();
        // Would sleep 30s without the shutdown flag; must return well under that.
        master.interruptible_sleep(Duration::from_secs(30));
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "interruptible_sleep did not honor shutdown flag: took {:?}",
            start.elapsed()
        );
    }
}
