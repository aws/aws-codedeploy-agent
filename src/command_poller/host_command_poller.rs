//! @risk medium
//!
//! Host command polling loop.
//!
//! Polls the `CodeDeploy` service for host commands and submits them
//! to the `CommandProcessor` for concurrent execution.
//!
//! ## Architecture notes
//!
//! - **Concurrent dispatch**: Commands are dispatched to a thread pool with a
//!   configurable concurrency limit (default 16), matching Ruby's
//!   `Concurrent::ThreadPoolExecutor(max_threads: 16, max_queue: 0)`.
//!   See `command_poller.rb#initialize`.
//! - **Graceful shutdown**: Uses `CancelToken` (`AtomicBool`) checked each iteration.
//!   On shutdown, waits for in-flight commands to complete.
//! - **Error handling**: Poll errors trigger backoff. Command execution errors
//!   are self-contained in the spawned thread and reported to the service —
//!   they do NOT affect the polling backoff (matches Ruby `base.rb#run`).

use super::CommandProcessor;
use super::backoff::PollBackoff;
use super::crash_recovery;
use crate::aws_clients::codedeploy_command_client::{CodeDeployCommandClient, HostCommand};
use crate::runtime::deployment_tracker::DeploymentTracker;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Maximum concurrent command processing threads.
///
/// Ruby: `command_poller.rb` — `Concurrent::ThreadPoolExecutor(max_threads: 16)`.
const MAX_CONCURRENT_COMMANDS: usize = 16;

/// Cancellation handle — call `cancel()` to stop the poll loop.
#[derive(Debug, Clone)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks in-flight command threads with a concurrency limit.
///
/// Ruby: `command_poller.rb` — `Concurrent::ThreadPoolExecutor` with
/// `max_threads: 16` and `max_queue: 0` (unbounded). Tasks are posted
/// via `@thread_pool.post { ... }` and never rejected unless the pool
/// is shut down. Graceful shutdown calls `@thread_pool.shutdown` then
/// `wait_for_termination(kill_agent_max_wait_time_seconds)`.
#[derive(Debug)]
struct CommandThreadPool {
    in_flight: Arc<AtomicUsize>,
    max_concurrent: usize,
}

impl CommandThreadPool {
    fn new(max_concurrent: usize) -> Self {
        Self { in_flight: Arc::new(AtomicUsize::new(0)), max_concurrent }
    }

    /// Returns `true` if the pool has capacity for another command.
    fn has_capacity(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed) < self.max_concurrent
    }

    /// Spawn a command processing thread. The in-flight counter is
    /// incremented before spawn and decremented when the thread exits.
    fn spawn<T: DeploymentTracker + Send + Sync + 'static>(
        &self,
        processor: Arc<CommandProcessor<T>>,
        command: HostCommand,
    ) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        let counter = Arc::clone(&self.in_flight);
        std::thread::spawn(move || {
            if let Err(e) = processor.process(&command) {
                error!("Command processing failed: {e}");
            }
            counter.fetch_sub(1, Ordering::Relaxed);
        });
    }

    /// Wait for all in-flight commands to complete, up to `timeout`.
    ///
    /// Ruby: `command_poller.rb#graceful_shutdown` —
    /// `@thread_pool.wait_for_termination(kill_agent_max_wait_time_seconds)`.
    fn wait_for_completion(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while self.in_flight.load(Ordering::Relaxed) > 0 {
            if Instant::now() >= deadline {
                warn!(
                    remaining = self.in_flight.load(Ordering::Relaxed),
                    "Shutdown timeout reached, {} commands still in flight",
                    self.in_flight.load(Ordering::Relaxed),
                );
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[derive(Debug)]
pub struct HostCommandPoller<T: DeploymentTracker + Send + Sync + 'static> {
    processor: Arc<CommandProcessor<T>>,
    client: CodeDeployCommandClient,
    tracker: Arc<T>,
    host_identifier: String,
    poll_interval: Duration,
    cancel: CancelToken,
    inject_dir: Option<PathBuf>,
    max_concurrent: usize,
    shutdown_timeout: Duration,
}

impl<T: DeploymentTracker + Send + Sync + 'static> HostCommandPoller<T> {
    #[must_use]
    pub fn new(
        client: CodeDeployCommandClient,
        processor: CommandProcessor<T>,
        tracker: T,
        host_identifier: String,
        cancel: CancelToken,
    ) -> Self {
        Self {
            processor: Arc::new(processor),
            client,
            tracker: Arc::new(tracker),
            host_identifier,
            poll_interval: DEFAULT_POLL_INTERVAL,
            cancel,
            inject_dir: None,
            max_concurrent: MAX_CONCURRENT_COMMANDS,
            shutdown_timeout: Duration::from_secs(7200),
        }
    }

    /// Set custom poll interval.
    #[must_use]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Set the directory for injected commands.
    #[must_use]
    pub fn with_inject_dir(mut self, dir: PathBuf) -> Self {
        self.inject_dir = Some(dir);
        self
    }

    /// Set the graceful shutdown timeout for in-flight commands.
    ///
    /// Ruby: `command_poller.rb#graceful_shutdown` uses
    /// `ProcessManager::Config.config[:kill_agent_max_wait_time_seconds]`.
    #[must_use]
    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// Entry point: recover from any crashed deployment, then poll in a loop.
    /// Returns when the cancellation token is cancelled.
    ///
    /// Ruby: `base.rb#run` — polls in a loop with exponential backoff on errors.
    /// `command_poller.rb#perform` — posts commands to a thread pool via
    /// `@thread_pool.post { acknowledge_and_process_command(command) }`.
    /// On success, sleeps `poll_interval`. On error, sleeps the backoff duration
    /// minus elapsed time.
    pub fn start(&self) {
        info!(
            host_identifier = %self.host_identifier,
            poll_interval_ms = self.poll_interval.as_millis().try_into().unwrap_or(u64::MAX),
            max_concurrent = self.max_concurrent,
            "Starting host command poll loop"
        );

        crash_recovery::recover(&self.client, self.tracker.as_ref());

        let mut backoff = PollBackoff::new();
        let pool = CommandThreadPool::new(self.max_concurrent);

        while !self.cancel.is_cancelled() {
            // Check for injected command from command port.
            // Injected commands run synchronously (same as Ruby — inject is
            // a diagnostic tool, not a production path).
            if let Some(command) = self.check_injected_command() {
                let result = self.processor.process(&command);
                self.write_inject_response(&result);
            }

            let start = Instant::now();

            match self.poll() {
                Ok(Some(command)) => {
                    backoff.reset();
                    // Ruby: `command_poller.rb#perform` — posts to thread pool.
                    // If pool is at capacity, skip this command. The service
                    // will re-send it on the next poll (commands are idempotent
                    // until acknowledged).
                    if pool.has_capacity() {
                        pool.spawn(Arc::clone(&self.processor), command);
                    } else {
                        warn!(
                            max_concurrent = self.max_concurrent,
                            "All command slots busy, deferring command"
                        );
                    }
                },
                Ok(None) => {
                    backoff.reset();
                },
                Err(e) => {
                    error!("Error polling for host commands: {e}");
                    backoff.record_error();
                },
            }

            let sleep = if backoff.error_count() > 0 {
                backoff.sleep_duration(start.elapsed()).unwrap_or(Duration::ZERO)
            } else {
                self.poll_interval
            };

            if !sleep.is_zero() {
                debug!(
                    sleep_secs = sleep.as_secs(),
                    error_count = backoff.error_count(),
                    "Sleeping"
                );
                std::thread::sleep(sleep);
            }
        }

        // Ruby: `command_poller.rb#graceful_shutdown` —
        // `@thread_pool.shutdown; @thread_pool.wait_for_termination(timeout)`
        info!("Polling loop stopped — waiting for in-flight commands");
        pool.wait_for_completion(self.shutdown_timeout);
        info!("All command threads finished — shutdown complete");
    }

    /// Poll the service for the next host command.
    ///
    /// Returns `Ok(Some(cmd))` on command, `Ok(None)` on empty poll, `Err` on failure.
    fn poll(&self) -> Result<Option<HostCommand>, String> {
        debug!("Calling PollHostCommand:");

        let output = self
            .client
            .poll_host_command(&self.host_identifier)
            .map_err(|e| e.to_string())?;

        let Some(command) = output else {
            debug!("PollHostCommand: Host Command = nil");
            return Ok(None);
        };

        debug!(
            "PollHostCommand: Host Identifier = {}; \
             Host Command Identifier = {}; \
             Deployment Execution ID = {}; \
             Command Name = {}",
            command.host_identifier,
            command.host_command_identifier,
            command.deployment_execution_id,
            command.command_name,
        );

        if let Err(e) = self.validate_command(&command) {
            error!("Invalid host command: {e}");
            return Ok(None);
        }

        Ok(Some(command))
    }

    /// Validate host identifier matches and command name is present.
    fn validate_command(&self, command: &HostCommand) -> Result<(), String> {
        if !self.host_identifier.contains(&command.host_identifier) {
            return Err(format!(
                "Host Identifier mismatch: {} != {}",
                self.host_identifier, command.host_identifier
            ));
        }

        if command.command_name.is_empty() {
            return Err("Command Name missing".to_string());
        }

        Ok(())
    }

    /// Check for an injected command file from the command port.
    fn check_injected_command(&self) -> Option<HostCommand> {
        let dir = self.inject_dir.as_ref()?;
        let cmd_path = dir.join(".injected-command.json");
        if !cmd_path.exists() {
            return None;
        }
        let content = std::fs::read_to_string(&cmd_path).ok()?;
        let _ = std::fs::remove_file(&cmd_path);
        let command: HostCommand = serde_json::from_str(&content).ok()?;
        info!(command_name = %command.command_name, "Processing injected command");
        Some(command)
    }

    /// Write the result of an injected command to the response file.
    fn write_inject_response(&self, result: &std::io::Result<()>) {
        let Some(dir) = self.inject_dir.as_ref() else {
            return;
        };
        let resp_path = dir.join(".injected-response.json");
        let resp = match result {
            Ok(()) => serde_json::json!({"ok": true}),
            Err(e) => serde_json::json!({"ok": false, "error": e.to_string()}),
        };
        let _ = std::fs::write(&resp_path, resp.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_clients::{CredentialMode, Credentials};
    use crate::host_command::{CommandDispatcher, DeploymentArchives};
    use crate::runtime::file_based_deployment_tracker::FileBasedDeploymentTracker;
    use crate::system::SystemFileOperations;
    use std::collections::HashMap;
    use tempfile::TempDir;

    fn test_client() -> CodeDeployCommandClient {
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "i-1234".into(),
            mode: CredentialMode::IamUser {
                access_key_id: "AKIATEST".into(),
                secret_access_key: "test-secret".into(),
            },
        };
        CodeDeployCommandClient::new(creds, false, false, None, Duration::from_secs(80), None)
            .unwrap()
    }

    fn test_poller(
        dir: &TempDir,
    ) -> HostCommandPoller<FileBasedDeploymentTracker<SystemFileOperations>> {
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&instructions).unwrap();
        let archives = Arc::new(DeploymentArchives::new(root, instructions, 5));
        let dispatcher = CommandDispatcher::new(archives, None, HashMap::new());
        let tracker = FileBasedDeploymentTracker::new(dir.path().join("tracking"));
        let client = test_client();
        let processor = CommandProcessor::new(
            test_client(),
            dispatcher,
            FileBasedDeploymentTracker::new(dir.path().join("tracking2")),
            "i-1234".into(),
        );

        HostCommandPoller::new(client, processor, tracker, "i-1234".into(), CancelToken::new())
    }

    fn test_command() -> HostCommand {
        HostCommand {
            host_identifier: "i-1234".into(),
            host_command_identifier: "cmd-1".into(),
            deployment_execution_id: "exec-1".into(),
            command_name: "AfterInstall".into(),
        }
    }

    #[test]
    fn validate_command_ok() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir);
        assert!(poller.validate_command(&test_command()).is_ok());
    }

    #[test]
    fn validate_command_host_mismatch() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir);
        let mut cmd = test_command();
        cmd.host_identifier = "i-9999".into();
        let err = poller.validate_command(&cmd).unwrap_err();
        assert!(err.contains("Host Identifier mismatch"));
    }

    #[test]
    fn validate_command_empty_name() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir);
        let mut cmd = test_command();
        cmd.command_name = String::new();
        let err = poller.validate_command(&cmd).unwrap_err();
        assert!(err.contains("Command Name missing"));
    }

    #[test]
    fn poll_returns_error_when_service_unreachable() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir);
        // Real client returns error (no service), poll() propagates it
        assert!(poller.poll().is_err());
    }

    #[test]
    fn with_poll_interval() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir).with_poll_interval(Duration::from_secs(5));
        assert_eq!(poller.poll_interval, Duration::from_secs(5));
    }

    #[test]
    fn cancel_token_new_is_not_cancelled() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancel_token_cancel_sets_flag() {
        let token = CancelToken::new();
        token.cancel();
        assert!(token.is_cancelled());
    }

    #[test]
    fn cancel_token_default() {
        let token = CancelToken::default();
        assert!(!token.is_cancelled());
    }

    #[test]
    fn cancel_token_clone_shares_state() {
        let token = CancelToken::new();
        let cloned = token.clone();
        token.cancel();
        assert!(cloned.is_cancelled());
    }

    #[test]
    fn cancel_token_is_debuggable() {
        let token = CancelToken::new();
        let debug = format!("{token:?}");
        assert!(debug.contains("CancelToken"));
    }

    #[test]
    fn start_exits_when_cancelled() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir);
        // Cancel immediately so the loop exits on first check
        poller.cancel.cancel();
        // Run on a background thread — the real client contains a
        // reqwest::blocking::Client which owns its own tokio runtime and
        // cannot be dropped inside another async runtime context.
        let handle = std::thread::spawn(move || poller.start());
        handle.join().unwrap();
        // If we get here, the loop exited correctly
    }

    #[test]
    fn with_shutdown_timeout() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir).with_shutdown_timeout(Duration::from_secs(60));
        assert_eq!(poller.shutdown_timeout, Duration::from_secs(60));
    }

    #[test]
    fn command_thread_pool_has_capacity_when_empty() {
        let pool = CommandThreadPool::new(16);
        assert!(pool.has_capacity());
    }

    #[test]
    fn command_thread_pool_tracks_in_flight() {
        let pool = CommandThreadPool::new(2);
        pool.in_flight.store(2, Ordering::Relaxed);
        assert!(!pool.has_capacity());
        pool.in_flight.store(1, Ordering::Relaxed);
        assert!(pool.has_capacity());
    }

    #[test]
    fn command_thread_pool_wait_returns_immediately_when_empty() {
        let pool = CommandThreadPool::new(16);
        let start = Instant::now();
        pool.wait_for_completion(Duration::from_secs(5));
        assert!(start.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn command_thread_pool_wait_respects_timeout() {
        let pool = CommandThreadPool::new(16);
        // Simulate a stuck in-flight command
        pool.in_flight.store(1, Ordering::Relaxed);
        let start = Instant::now();
        pool.wait_for_completion(Duration::from_millis(300));
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(250), "should wait near timeout");
        assert!(elapsed < Duration::from_millis(600), "should not wait much beyond timeout");
    }

    #[test]
    fn command_thread_pool_spawn_increments_and_decrements() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&instructions).unwrap();
        let archives = Arc::new(DeploymentArchives::new(root, instructions, 5));
        let dispatcher = CommandDispatcher::new(archives, None, HashMap::new());
        let processor = Arc::new(CommandProcessor::new(
            test_client(),
            dispatcher,
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().join("tracking")),
            "i-1234".into(),
        ));

        let pool = CommandThreadPool::new(16);
        assert_eq!(pool.in_flight.load(Ordering::Relaxed), 0);

        // Spawn a command — it will fail (no service) but the counter should
        // increment then decrement.
        pool.spawn(processor, test_command());

        // Wait for the thread to finish
        pool.wait_for_completion(Duration::from_secs(5));
        assert_eq!(pool.in_flight.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn default_max_concurrent_is_16() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir);
        assert_eq!(poller.max_concurrent, 16);
    }

    #[test]
    fn check_injected_command_no_dir() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir); // inject_dir is None
        assert!(poller.check_injected_command().is_none());
    }

    #[test]
    fn check_injected_command_no_file() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir).with_inject_dir(dir.path().to_path_buf());
        assert!(poller.check_injected_command().is_none());
    }

    #[test]
    fn check_injected_command_reads_and_deletes() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir).with_inject_dir(dir.path().to_path_buf());
        let cmd_path = dir.path().join(".injected-command.json");
        std::fs::write(
            &cmd_path,
            r#"{"host_identifier":"i-1234","host_command_identifier":"cmd-1","deployment_execution_id":"exec-1","command_name":"Install"}"#,
        ).unwrap();
        let cmd = poller.check_injected_command().unwrap();
        assert_eq!(cmd.command_name, "Install");
        assert!(!cmd_path.exists(), "command file should be deleted");
    }

    #[test]
    fn check_injected_command_invalid_json() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir).with_inject_dir(dir.path().to_path_buf());
        std::fs::write(dir.path().join(".injected-command.json"), "not json").unwrap();
        assert!(poller.check_injected_command().is_none());
    }

    #[test]
    fn write_inject_response_ok() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir).with_inject_dir(dir.path().to_path_buf());
        poller.write_inject_response(&Ok(()));
        let resp: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".injected-response.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(resp["ok"], true);
    }

    #[test]
    fn write_inject_response_err() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir).with_inject_dir(dir.path().to_path_buf());
        poller.write_inject_response(&Err(std::io::Error::other("boom")));
        let resp: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".injected-response.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(resp["ok"], false);
        assert!(resp["error"].as_str().unwrap().contains("boom"));
    }

    #[test]
    fn write_inject_response_no_dir() {
        let dir = TempDir::new().unwrap();
        let poller = test_poller(&dir); // inject_dir is None
        poller.write_inject_response(&Ok(())); // should not panic
    }
}
