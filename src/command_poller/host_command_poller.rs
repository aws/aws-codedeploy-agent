//! Host command polling loop.
//!
//! Polls the `CodeDeploy` service for host commands and submits them
//! to the `CommandProcessor` for concurrent execution.
//!
//! ## Architecture notes
//!
//! - **Concurrent dispatch**: Commands are dispatched to a thread pool with a
//!   configurable concurrency limit (default 16) and no queue — a command is
//!   either dispatched to a free thread or left for the next poll.
//! - **Graceful shutdown**: Uses `CancelToken` (`AtomicBool`) checked each iteration.
//!   On shutdown, waits for in-flight commands to complete.
//! - **Error handling**: Poll errors trigger backoff. Command execution errors
//!   are self-contained in the spawned thread and reported to the service —
//!   they do NOT affect the polling backoff.

use super::CommandProcessor;
use super::backoff::PollBackoff;
use super::crash_recovery;
use crate::aws_clients::codedeploy_command_client::{CodeDeployCommandClient, HostCommand};
use crate::runtime::deployment_tracker::DeploymentTracker;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tracing::{debug, error, info, warn};

const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Maximum concurrent command processing threads.
const MAX_CONCURRENT_COMMANDS: usize = 16;

/// How many consecutive dedup-skips (re-deliveries of in-flight commands) may
/// re-poll immediately before the loop falls through to the poll-interval
/// sleep. One extra poll catches another deployment's command queued behind a
/// duplicate; capping the run prevents hot-spinning `PollHostCommand` when the
/// service repeatedly (or alternately) re-serves commands whose acks are still
/// in flight.
const MAX_CONSECUTIVE_DEDUP_REPOLLS: u32 = 1;

/// What the poll loop should do after a `PollHostCommand` that returned a
/// command. Extracted from `start()` so the branch logic is unit-testable
/// without a live service, tokio runtime, or cancel-token wiring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PollAction {
    /// The command is already being processed (a service re-delivery whose ack
    /// has not landed). Skip it without spawning. `repoll` = re-poll
    /// immediately (a different deployment's command may be queued behind the
    /// duplicate); `!repoll` = fall through to the poll-interval sleep to avoid
    /// hot-spinning `PollHostCommand`.
    SkipDuplicate { repoll: bool },
    /// Spawn the command. `repoll` = re-poll immediately (poll-when-pending);
    /// `!repoll` = sleep (the throttle gate is open).
    Dispatch { repoll: bool },
    /// The thread pool is at capacity — defer (warn + sleep).
    DeferAtCapacity,
}

/// Decide what to do with a polled command, given the current pool state and
/// consecutive-dedup-skip count. Pure function: no I/O, so it is exhaustively
/// unit-testable. `start()` maps the result to spawn / continue / sleep and
/// owns the side effects (spawning, the skip counter, logging).
///
/// `consecutive_dedup_skips` is the count INCLUDING the current skip when the
/// command is a duplicate (i.e. the caller increments before calling for the
/// duplicate case); it is ignored otherwise.
fn decide_poll_action(
    is_duplicate: bool,
    has_capacity: bool,
    throttled: bool,
    consecutive_dedup_skips: u32,
) -> PollAction {
    if is_duplicate {
        let repoll = consecutive_dedup_skips <= MAX_CONSECUTIVE_DEDUP_REPOLLS && !throttled;
        PollAction::SkipDuplicate { repoll }
    } else if has_capacity {
        PollAction::Dispatch { repoll: !throttled }
    } else {
        PollAction::DeferAtCapacity
    }
}

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

/// Releases a command's in-flight slot and identifier when dropped, so the
/// cleanup runs even if the processing thread panics (not just on the normal
/// return path). See `CommandThreadPool::spawn`.
struct InFlightGuard {
    ids: Arc<Mutex<HashSet<String>>>,
    counter: Arc<AtomicUsize>,
    id: String,
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.ids.lock().unwrap_or_else(PoisonError::into_inner).remove(&self.id);
        self.counter.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Tracks in-flight command threads with a concurrency limit.
///
/// Holds up to `max_concurrent` threads with no queue; graceful shutdown waits
/// for in-flight threads up to `kill_agent_max_wait_time_seconds`.
#[derive(Debug)]
struct CommandThreadPool {
    in_flight: Arc<AtomicUsize>,
    /// `host_command_identifier`s currently being processed. Used to skip
    /// service re-deliveries of a command we already hold: the service keeps
    /// serving a command until its acknowledgement lands, and the
    /// poll-when-pending immediate re-poll usually wins that race, so without
    /// this each command arrives 2–3 times, each duplicate burning a thread
    /// slot on a spec-fetch + losing `Failed` ack. Bounded by
    /// `max_concurrent`.
    in_flight_ids: Arc<Mutex<HashSet<String>>>,
    max_concurrent: usize,
}

impl CommandThreadPool {
    fn new(max_concurrent: usize) -> Self {
        Self {
            in_flight: Arc::new(AtomicUsize::new(0)),
            in_flight_ids: Arc::new(Mutex::new(HashSet::new())),
            max_concurrent,
        }
    }

    /// Returns `true` if the pool has capacity for another command.
    fn has_capacity(&self) -> bool {
        self.in_flight.load(Ordering::Relaxed) < self.max_concurrent
    }

    /// Returns `true` if a command with this identifier is being processed.
    fn is_in_flight(&self, host_command_identifier: &str) -> bool {
        self.in_flight_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(host_command_identifier)
    }

    /// Spawn a command processing thread. The in-flight counter and identifier
    /// set are updated before spawn and cleared when the thread exits.
    fn spawn<T: DeploymentTracker + Send + Sync + 'static>(
        &self,
        processor: Arc<CommandProcessor<T>>,
        command: HostCommand,
    ) {
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        self.in_flight_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(command.host_command_identifier.clone());
        // Cleanup runs via a drop guard, not inline after `process()`, so a
        // panic in `process()` still releases the slot and clears the
        // in-flight identifier. Without this, a panicking command would leave
        // its identifier in `in_flight_ids` forever, permanently dedup-skipping
        // every future re-delivery of that command until an agent restart.
        let guard = InFlightGuard {
            ids: Arc::clone(&self.in_flight_ids),
            counter: Arc::clone(&self.in_flight),
            id: command.host_command_identifier.clone(),
        };
        std::thread::spawn(move || {
            let _guard = guard;
            if let Err(e) = processor.process(&command) {
                error!("Command processing failed: {e}");
            }
        });
    }

    /// Wait for all in-flight commands to complete, up to `timeout`.
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
            shutdown_timeout: Duration::from_hours(2),
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

    /// Set the graceful shutdown timeout for in-flight commands, from the
    /// `kill_agent_max_wait_time_seconds` config option.
    #[must_use]
    pub fn with_shutdown_timeout(mut self, timeout: Duration) -> Self {
        self.shutdown_timeout = timeout;
        self
    }

    /// Entry point: recover from any crashed deployment, then poll in a loop.
    /// Returns when the cancellation token is cancelled.
    ///
    /// Polls in a loop with exponential backoff on errors, posting each command
    /// to the thread pool for acknowledgement and processing. On success, sleeps
    /// `poll_interval`. On error, sleeps the backoff duration minus elapsed
    /// time.
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
        // Count of consecutive dedup-skips (re-deliveries of commands already
        // in flight). The first skip in a run re-polls immediately — a
        // different deployment's command may be queued behind the duplicate —
        // but after `MAX_CONSECUTIVE_DEDUP_REPOLLS` we fall through to the
        // poll-interval sleep instead of hot-spinning PollHostCommand while
        // acks are still in flight. A count (not a last-identifier marker) is
        // required because the service can alternate re-deliveries of several
        // distinct in-flight commands (A, B, A, B, …); a single-identifier
        // marker would never see a "repeat" and would re-poll forever.
        let mut consecutive_dedup_skips: u32 = 0;

        while !self.cancel.is_cancelled() {
            // Check for injected command from command port.
            // Injected commands run synchronously — inject is a diagnostic
            // tool, not a production path.
            if let Some(command) = self.check_injected_command() {
                let result = self.processor.process(&command);
                self.write_inject_response(&result);
            }

            let start = Instant::now();

            match self.poll() {
                Ok(Some(command)) => {
                    backoff.reset();
                    let is_duplicate = pool.is_in_flight(&command.host_command_identifier);
                    // The skip counter must be incremented BEFORE deciding, so
                    // the cap sees this skip; reset to 0 on any non-skip.
                    if is_duplicate {
                        consecutive_dedup_skips += 1;
                    } else {
                        consecutive_dedup_skips = 0;
                    }
                    let throttled = self.client.throttle_gate().is_throttled();
                    match decide_poll_action(
                        is_duplicate,
                        pool.has_capacity(),
                        throttled,
                        consecutive_dedup_skips,
                    ) {
                        PollAction::SkipDuplicate { repoll } => {
                            // Service re-delivery of a command we are already
                            // processing (its ack has not landed). Do NOT spawn
                            // a second thread to lose the ack race.
                            debug!(
                                command_name = %command.command_name,
                                consecutive_dedup_skips,
                                repoll,
                                "Command already in flight, skipping re-delivery"
                            );
                            if repoll {
                                continue;
                            }
                        },
                        PollAction::Dispatch { repoll } => {
                            // Commands are idempotent until acknowledged.
                            pool.spawn(Arc::clone(&self.processor), command);
                            // Poll-when-pending: re-poll immediately while
                            // commands are available and the pool has capacity,
                            // unless the throttle gate is open.
                            if repoll {
                                debug!("Command dispatched, re-polling immediately");
                                continue;
                            }
                            debug!("Command dispatched but throttle gate is open, sleeping");
                        },
                        PollAction::DeferAtCapacity => {
                            warn!(
                                max_concurrent = self.max_concurrent,
                                "All command slots busy, deferring command"
                            );
                        },
                    }
                },
                Ok(None) => {
                    consecutive_dedup_skips = 0;
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
    use crate::config::AgentConfig;
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
        let dispatcher = CommandDispatcher::new(
            archives,
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
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
        let poller = test_poller(&dir).with_shutdown_timeout(Duration::from_mins(1));
        assert_eq!(poller.shutdown_timeout, Duration::from_mins(1));
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
        let dispatcher = CommandDispatcher::new(
            archives,
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
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

    // === Poll-When-Pending — concurrent dispatch tests ===

    /// Verifies multiple commands dispatched concurrently all complete
    /// independently. Proves the thread pool handles parallel execution
    /// without blocking or corruption.
    #[test]
    fn concurrent_dispatch_completes_independently() {
        let dir = TempDir::new().unwrap();
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&instructions).unwrap();
        let archives = Arc::new(DeploymentArchives::new(root, instructions, 5));
        let dispatcher = CommandDispatcher::new(
            archives,
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        let processor = Arc::new(CommandProcessor::new(
            test_client(),
            dispatcher,
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().join("tracking")),
            "i-1234".into(),
        ));

        let pool = CommandThreadPool::new(16);

        // Dispatch 5 commands rapidly (simulating poll-when-pending behavior
        // where the loop continues without sleeping between dispatches)
        for i in 0..5 {
            assert!(pool.has_capacity(), "pool should have capacity for command {i}");
            pool.spawn(
                Arc::clone(&processor),
                HostCommand {
                    host_identifier: "i-1234".into(),
                    host_command_identifier: format!("cmd-{i}"),
                    deployment_execution_id: format!("exec-{i}"),
                    command_name: "AfterInstall".into(),
                },
            );
        }

        // All 5 should complete independently (they'll error — no service —
        // but that's fine; we're testing the dispatch mechanism)
        pool.wait_for_completion(Duration::from_secs(10));
        assert_eq!(pool.in_flight.load(Ordering::Relaxed), 0);
    }

    /// Verifies capacity gating: when pool is full, `has_capacity()` returns
    /// false, preventing the poll-when-pending `continue` from firing (the
    /// loop falls through to sleep instead).
    #[test]
    fn pool_at_capacity_blocks_further_dispatch() {
        let pool = CommandThreadPool::new(3);
        // Simulate 3 in-flight commands
        pool.in_flight.store(3, Ordering::Relaxed);
        assert!(!pool.has_capacity());

        // One finishes
        pool.in_flight.store(2, Ordering::Relaxed);
        assert!(pool.has_capacity());
    }

    /// Documents the poll-when-pending behavior:
    /// - Command received + `has_capacity` → spawn + continue (no sleep)
    /// - Command received + at capacity → warn + `sleep(poll_interval)`
    /// - No command → `sleep(poll_interval)`
    /// - Error → sleep(backoff)
    ///
    /// The `continue` after `pool.spawn()` in `start()` implements this.
    /// Full integration coverage: `scripts/e2e-ec2-concurrent.sh` exercises
    /// N parallel deployments against the same instance, verifying commands
    /// for distinct deployment groups are fetched and dispatched concurrently.
    #[test]
    fn poll_when_pending_capacity_check_gates_continue() {
        // This test verifies the precondition: has_capacity must be true
        // for the continue path to fire. When false, the loop sleeps.
        let pool = CommandThreadPool::new(2);

        // Empty pool: continue path fires
        assert!(pool.has_capacity());

        // 1 in-flight: continue path still fires
        pool.in_flight.store(1, Ordering::Relaxed);
        assert!(pool.has_capacity());

        // 2 in-flight (at max): continue path does NOT fire, loop sleeps
        pool.in_flight.store(2, Ordering::Relaxed);
        assert!(!pool.has_capacity());
    }

    /// A service re-delivery of a command whose thread is still running must
    /// be detectable so the loop skips it instead of spawning a losing
    /// duplicate (the ack race: re-poll-when-pending usually beats the
    /// executing thread's acknowledgement).
    #[test]
    fn in_flight_identifier_tracked_while_processing() {
        let pool = CommandThreadPool::new(2);

        assert!(!pool.is_in_flight("cmd-1"), "nothing in flight initially");

        pool.in_flight_ids.lock().unwrap().insert("cmd-1".to_string());
        assert!(pool.is_in_flight("cmd-1"), "identifier registered while processing");
        assert!(!pool.is_in_flight("cmd-2"), "other identifiers unaffected");

        // Thread completion removes the identifier — a later legitimate
        // re-delivery (lost ack, service retry) is processed again.
        pool.in_flight_ids.lock().unwrap().remove("cmd-1");
        assert!(!pool.is_in_flight("cmd-1"), "identifier cleared on completion");
    }

    /// A panic in the processing thread must still release the in-flight slot
    /// and identifier (via `InFlightGuard::drop`) — otherwise a panicking
    /// command would be dedup-skipped forever. Simulates the guard's lifetime
    /// inside a thread that panics.
    #[test]
    fn panicking_thread_releases_slot_via_guard() {
        let ids = Arc::new(Mutex::new(HashSet::new()));
        let counter = Arc::new(AtomicUsize::new(1));
        ids.lock().unwrap().insert("cmd-boom".to_string());

        let ids2 = Arc::clone(&ids);
        let counter2 = Arc::clone(&counter);
        let handle = std::thread::spawn(move || {
            let _guard = InFlightGuard { ids: ids2, counter: counter2, id: "cmd-boom".to_string() };
            panic!("simulated processor panic");
        });
        assert!(handle.join().is_err(), "thread should have panicked");

        assert!(
            !ids.lock().unwrap().contains("cmd-boom"),
            "identifier must be cleared even when the thread panics"
        );
        assert_eq!(counter.load(Ordering::Relaxed), 0, "slot must be released on panic");
    }

    // --- decide_poll_action ---
    // These call the real decision function `start()` uses, so a change to the
    // loop's branch logic is caught here (the previous test mirrored the rule
    // in a local closure and could not).

    /// A command with pool capacity dispatches; it re-polls immediately unless
    /// the throttle gate is open.
    #[test]
    fn decide_dispatches_with_capacity() {
        assert_eq!(
            decide_poll_action(false, true, false, 0),
            PollAction::Dispatch { repoll: true }
        );
        assert_eq!(
            decide_poll_action(false, true, true, 0),
            PollAction::Dispatch { repoll: false },
            "throttle gate open ⇒ dispatch but do not immediate-re-poll"
        );
    }

    /// A non-duplicate command with no capacity defers regardless of throttle.
    #[test]
    fn decide_defers_when_at_capacity() {
        assert_eq!(decide_poll_action(false, false, false, 0), PollAction::DeferAtCapacity);
        assert_eq!(decide_poll_action(false, false, true, 0), PollAction::DeferAtCapacity);
    }

    /// A duplicate re-delivery skips; the first skip in a run re-polls, but past
    /// the cap it sleeps — and this holds regardless of identity, so alternating
    /// distinct duplicates (A, B, A, …) still converge instead of hot-spinning
    /// `PollHostCommand`. `has_capacity` is irrelevant for a duplicate.
    #[test]
    fn decide_skips_duplicate_and_caps_repolls() {
        // 1st skip in a run: re-poll.
        assert_eq!(
            decide_poll_action(true, true, false, 1),
            PollAction::SkipDuplicate { repoll: true }
        );
        // Beyond the cap: skip WITHOUT re-poll (sleep), even with capacity.
        for skips in 2..=6 {
            assert_eq!(
                decide_poll_action(true, true, false, skips),
                PollAction::SkipDuplicate { repoll: false },
                "skip #{skips} must sleep, not hot-spin"
            );
        }
    }

    /// A throttled duplicate never re-polls, even on its first skip — the gate
    /// backoff must apply.
    #[test]
    fn decide_skips_duplicate_without_repoll_when_throttled() {
        assert_eq!(
            decide_poll_action(true, true, true, 1),
            PollAction::SkipDuplicate { repoll: false }
        );
    }
}
