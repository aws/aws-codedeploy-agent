//! Processes a single host command end-to-end.
//!
//! Orchestrates the full lifecycle of a deployment command: fetch the deployment
//! specification, acknowledge receipt, execute the command (download/install/hook),
//! and report the result back to the `CodeDeploy` service.

use super::diagnostics;
use crate::aws_clients::codedeploy_command_client::{
    AckStatus, CodeDeployClientError, CodeDeployCommandClient, DeploymentSpecification, HostCommand,
};
use crate::deployment_specification::{self, DeploymentSpec};
use crate::host_command::CommandDispatcher;
use crate::lifecycle_event::ScriptError;
use crate::runtime::deployment_tracker::DeploymentTracker;
use std::io;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, info_span, warn};

/// How long to keep retrying `PutHostCommandComplete` on transient errors.
///
/// The service fails a lifecycle event 70 minutes after it was acknowledged
/// if no completion arrives, and discards any completion sent after that. Retrying is safe before then: the service keeps
/// the first completion it records and discards later duplicates.
#[derive(Debug, Clone, Copy)]
pub struct CompletionRetryPolicy {
    /// Stop retrying this long after the command was acknowledged.
    pub deadline_after_ack: Duration,
    /// Backoff ceiling for the first retry; doubles each attempt.
    pub initial_backoff: Duration,
    /// Upper bound on any single backoff.
    pub max_backoff: Duration,
}

impl Default for CompletionRetryPolicy {
    fn default() -> Self {
        Self {
            // 5 minutes inside the service's 70-minute timeout.
            deadline_after_ack: Duration::from_mins(65),
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
        }
    }
}

impl CompletionRetryPolicy {
    /// Full-jitter exponential backoff for retry number `attempt` (1-based).
    fn backoff(&self, attempt: u32) -> Duration {
        let ceiling = self
            .initial_backoff
            .saturating_mul(2u32.saturating_pow(attempt.saturating_sub(1)))
            .min(self.max_backoff);
        let ceiling_ms = u64::try_from(ceiling.as_millis()).unwrap_or(u64::MAX);
        if ceiling_ms == 0 {
            return Duration::ZERO;
        }
        let mut buf = [0u8; 8];
        // getrandom never fails on supported platforms; fall back to the ceiling.
        if getrandom::getrandom(&mut buf).is_err() {
            return ceiling;
        }
        Duration::from_millis(u64::from_le_bytes(buf) % (ceiling_ms + 1))
    }
}

/// Trait abstracting the `CodeDeploy` service client for testability.
pub trait CommandServiceClient {
    /// # Errors
    /// Returns error if the service call fails.
    fn get_deployment_specification(
        &self,
        deployment_execution_id: &str,
        host_identifier: &str,
    ) -> Result<DeploymentSpecification, CodeDeployClientError>;

    /// # Errors
    /// Returns error if the service call fails.
    fn put_host_command_acknowledgement(
        &self,
        host_command_identifier: &str,
        diagnostics_payload: &str,
    ) -> Result<AckStatus, CodeDeployClientError>;

    /// # Errors
    /// Returns error if the service call fails.
    fn put_host_command_complete(
        &self,
        host_command_identifier: &str,
        status: &str,
        diagnostics_payload: &str,
    ) -> Result<(), CodeDeployClientError>;

    /// Drop pooled connections so the next call can reach a different backend
    /// host. Best effort and may be rate-limited; the default does nothing.
    ///
    /// # Errors
    /// Returns error if the connections could not be reset.
    fn reset_connections(&self) -> Result<(), CodeDeployClientError> {
        Ok(())
    }
}

impl CommandServiceClient for CodeDeployCommandClient {
    fn get_deployment_specification(
        &self,
        deployment_execution_id: &str,
        host_identifier: &str,
    ) -> Result<DeploymentSpecification, CodeDeployClientError> {
        self.get_deployment_specification(deployment_execution_id, host_identifier)
    }

    fn put_host_command_acknowledgement(
        &self,
        host_command_identifier: &str,
        diagnostics_payload: &str,
    ) -> Result<AckStatus, CodeDeployClientError> {
        self.put_host_command_acknowledgement(host_command_identifier, diagnostics_payload)
    }

    fn put_host_command_complete(
        &self,
        host_command_identifier: &str,
        status: &str,
        diagnostics_payload: &str,
    ) -> Result<(), CodeDeployClientError> {
        self.put_host_command_complete(host_command_identifier, status, diagnostics_payload)
    }

    fn reset_connections(&self) -> Result<(), CodeDeployClientError> {
        // Inherent method; it reports whether the rate limit allowed a reset.
        CodeDeployCommandClient::reset_connections(self).map(|_| ())
    }
}

#[derive(Debug)]
pub struct CommandProcessor<T: DeploymentTracker, C: CommandServiceClient = CodeDeployCommandClient>
{
    client: C,
    dispatcher: CommandDispatcher,
    tracker: T,
    host_identifier: String,
    deployment_system: String,
    completion_retry: CompletionRetryPolicy,
}

impl<T: DeploymentTracker, C: CommandServiceClient> CommandProcessor<T, C> {
    #[must_use]
    pub fn new(
        client: C,
        dispatcher: CommandDispatcher,
        tracker: T,
        host_identifier: String,
    ) -> Self {
        Self {
            client,
            dispatcher,
            tracker,
            host_identifier,
            deployment_system: "CodeDeploy".to_string(),
            completion_retry: CompletionRetryPolicy::default(),
        }
    }

    /// Override how completion reports are retried (tests use short durations).
    #[must_use]
    pub fn with_completion_retry_policy(mut self, policy: CompletionRetryPolicy) -> Self {
        self.completion_retry = policy;
        self
    }

    /// Process a single host command: get spec, ack, execute, report.
    ///
    /// # Errors
    /// Returns an error if any step fails fatally.
    pub fn process(&self, command: &HostCommand) -> io::Result<()> {
        info!(
            command_name = %command.command_name,
            deployment_execution_id = %command.deployment_execution_id,
            host_command_id = %command.host_command_identifier,
            "Processing host command"
        );

        let (envelope, format) = self.get_deployment_spec(command)?;
        let spec = Self::parse_spec(&envelope, &format)?;

        let _span = info_span!(
            "deployment",
            deployment_id = %spec.deployment_id,
            command = %command.command_name,
            app = %spec.application_name,
        )
        .entered();

        info!(
            deployment_group = %spec.deployment_group_name,
            "Deployment spec parsed"
        );

        let is_noop = self.dispatcher.is_command_noop(&command.command_name, &spec);
        // Taken before the ack is sent so the deadline can only err early: the
        // service starts its completion timeout when it records the ack.
        let report_deadline = Instant::now() + self.completion_retry.deadline_after_ack;
        let status = self.send_acknowledgement(command, is_noop)?;

        // Wrong match here skips execution or double-executes.
        match status {
            AckStatus::InProgress => {},
            AckStatus::Failed if is_noop => {
                // The service was already acked — re-complete as noop.
                // We reuse the already-parsed spec since it should not change between calls.
                self.report_completion(
                    command,
                    "Succeeded",
                    &diagnostics::success("CompletedNoopCommand"),
                    report_deadline,
                );
                return Ok(());
            },
            _ => return Ok(()),
        }

        self.execute_and_report(command, &spec, report_deadline)
    }

    fn get_deployment_spec(&self, command: &HostCommand) -> io::Result<(String, String)> {
        debug!(
            deployment_execution_id = %command.deployment_execution_id,
            "Fetching deployment specification"
        );

        let output = self
            .client
            .get_deployment_specification(&command.deployment_execution_id, &self.host_identifier)
            .map_err(|e| io::Error::other(format!("GetDeploymentSpecification failed: {e}")))?;

        debug!("GetDeploymentSpecification: Deployment System = {}", output.deployment_system);

        if output.deployment_system != self.deployment_system {
            return Err(io::Error::other(format!(
                "Deployment System mismatch: {} != {}",
                self.deployment_system, output.deployment_system
            )));
        }

        Ok((output.generic_envelope, output.envelope_format))
    }

    fn parse_spec(payload: &str, format: &str) -> io::Result<DeploymentSpec> {
        let envelope = deployment_specification::Envelope {
            format: format.to_string(),
            payload: payload.to_string(),
        };
        DeploymentSpec::parse(&envelope)
            .map_err(|e| io::Error::other(format!("Failed to parse deployment spec: {e}")))
    }

    /// Send ack to service with noop diagnostics. Returns the service's response status.
    fn send_acknowledgement(&self, command: &HostCommand, is_noop: bool) -> io::Result<AckStatus> {
        let noop_json = serde_json::json!({"IsCommandNoop": is_noop}).to_string();

        debug!("Calling PutHostCommandAcknowledgement:");

        let status = self
            .client
            .put_host_command_acknowledgement(&command.host_command_identifier, &noop_json)
            .map_err(|e| io::Error::other(format!("PutHostCommandAcknowledgement failed: {e}")))?;

        debug!("Command Status = {status:?}");

        // A `Failed` ack status does NOT mean a lifecycle phase failed on this
        // host. The service may return `Failed` for a command it still considers
        // in progress (e.g. after a server-side timeout). On a `Failed` ack the
        // agent checks whether the command is a noop and, if so, completes it
        // (see the match in `process`) to unblock the deployment immediately
        // instead of waiting out the full server-side timeout.
        //
        // Logged at INFO: the "checking whether command is a noop" tail makes
        // clear it is a recovery step, not a failure, and keeping it at INFO
        // preserves the operator signal — frequent occurrences indicate network
        // or thread-pool contention worth investigating.
        if matches!(status, AckStatus::Failed) {
            info!(
                "Received Failed for command {}, checking whether command is a noop...",
                command.command_name
            );
        }

        Ok(status)
    }

    fn execute_and_report(
        &self,
        command: &HostCommand,
        spec: &DeploymentSpec,
        report_deadline: Instant,
    ) -> io::Result<()> {
        self.tracker
            .start_tracking(&spec.deployment_id, &command.host_command_identifier)
            .map_err(|e| io::Error::other(format!("Failed to start deployment tracking: {e}")))?;

        info!("Executing command");

        let start = Instant::now();
        let result = self.dispatcher.execute_command(&command.command_name, spec);
        let elapsed = start.elapsed();

        // Wrong status string silently misreports the deployment outcome.
        match &result {
            Ok(_) => {
                info!(
                    elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX),
                    "Command succeeded"
                );
                let note = self.completion_note(&command.command_name, spec);
                self.report_completion(
                    command,
                    "Succeeded",
                    &diagnostics::success(&note),
                    report_deadline,
                );
            },
            Err(e) => {
                error!(
                    elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX),
                    error = %e,
                    "Command failed"
                );
                // A hook failure carries a `ScriptError` as the io::Error source;
                // recover it for the real error code + log tail.
                let payload = match e.get_ref().and_then(|s| s.downcast_ref::<ScriptError>()) {
                    Some(script_err) => diagnostics::from_script_error(script_err),
                    None => diagnostics::from_error(e),
                };
                self.report_completion(command, "Failed", &payload, report_deadline);
            },
        }

        if let Err(e) = self.tracker.stop_tracking(&spec.deployment_id) {
            error!("Failed to stop deployment tracking: {e}");
        }

        result.map(|_| ())
    }

    /// Detail the service can parse off a successful completion. Empty for commands that have none,
    /// which keeps their diagnostics byte for byte what they were.
    ///
    /// Read back from the `.bundle-source` marker `DownloadBundle` has just written, rather than
    /// returned up through the dispatcher: the marker is already the single record of where the
    /// bundle came from, so reading it cannot disagree with what the host actually did. A missing
    /// marker yields no note -- writing it is best-effort, and a metric is not worth failing a
    /// deployment over.
    fn completion_note(&self, command_name: &str, spec: &DeploymentSpec) -> String {
        if command_name != "DownloadBundle" {
            return String::new();
        }

        let marker = self
            .dispatcher
            .archives()
            .deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id)
            .join(crate::host_command::BUNDLE_SOURCE_FILE);

        match std::fs::read_to_string(&marker) {
            Ok(source) => crate::host_command::bundle_source_note(source.trim()),
            Err(e) => {
                debug!(path = %marker.display(), "No bundle source marker to report: {e}");
                String::new()
            },
        }
    }

    /// Report the command result, retrying transient failures until `deadline`.
    ///
    /// A lost completion leaves the lifecycle event `InProgress` until the
    /// service's 70-minute timeout fails it, so a brief 5xx burst from one
    /// backend host must not drop the result. Non-retryable errors (4xx) are
    /// logged and dropped.
    ///
    /// Retries deliberately continue through agent shutdown: the worker's drain
    /// (`kill_agent_max_wait_time_seconds`, default 2h, same as a running hook)
    /// waits for them, so a stop during a backend outage can take up to the
    /// deadline. Stopping early would be worse while the result lives only in
    /// memory: either it is lost (a 70-minute stall), or the tracking file
    /// survives and crash recovery reports `Failed` for a hook that succeeded.
    /// Once the result is persisted for replay after restart, this loop should
    /// honour the cancel token instead.
    fn report_completion(
        &self,
        command: &HostCommand,
        status: &str,
        payload: &str,
        deadline: Instant,
    ) {
        let host_command_id = &command.host_command_identifier;
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            debug!(attempt, "Calling PutHostCommandComplete: \"{status}\"");
            let err = match self.client.put_host_command_complete(host_command_id, status, payload)
            {
                Ok(()) => {
                    if attempt > 1 {
                        info!(attempt, %host_command_id, "Reported command completion after retrying");
                    }
                    return;
                },
                Err(e) => e,
            };

            let now = Instant::now();
            if !err.is_retryable() || now >= deadline {
                error!(
                    attempt,
                    %host_command_id,
                    retryable = err.is_retryable(),
                    "Failed to report command completion: {err}"
                );
                return;
            }

            let wait = self.completion_retry.backoff(attempt).min(deadline - now);
            warn!(
                attempt,
                %host_command_id,
                wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX),
                "Transient error reporting command completion, retrying: {err}"
            );
            // A 5xx response may come from one bad backend host that the pooled
            // keep-alive connection is pinned to; reconnect so the retry can land
            // elsewhere. Network errors and timeouts already get a new
            // connection, and throttling is not host-specific. The client
            // rate-limits resets so a fleet-wide outage doesn't become a
            // handshake storm.
            if err.is_server_error_response()
                && let Err(e) = self.client.reset_connections()
            {
                warn!("Failed to reset connections before retry: {e}");
            }
            std::thread::sleep(wait);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_clients::{CredentialMode, Credentials};
    use crate::config::AgentConfig;
    use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
    use crate::host_command::DeploymentArchives;
    use crate::runtime::file_based_deployment_tracker::FileBasedDeploymentTracker;
    use crate::system::SystemFileOperations;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;
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

    fn test_dispatcher(dir: &TempDir) -> CommandDispatcher {
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&instructions).unwrap();
        let archives = Arc::new(DeploymentArchives::new(root, instructions, 5));
        CommandDispatcher::new(
            archives,
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        )
    }

    fn test_tracker(dir: &TempDir) -> FileBasedDeploymentTracker<SystemFileOperations> {
        let tracking = dir.path().join("tracking");
        FileBasedDeploymentTracker::new(tracking)
    }

    fn test_command() -> HostCommand {
        HostCommand {
            host_identifier: "i-1234".into(),
            host_command_identifier: "cmd-1".into(),
            deployment_execution_id: "exec-1".into(),
            command_name: "AfterInstall".into(),
        }
    }

    fn test_spec() -> DeploymentSpec {
        DeploymentSpec {
            deployment_id: "d-123".into(),
            deployment_group_id: "dg-1".into(),
            deployment_group_name: "my-group".into(),
            application_name: "my-app".into(),
            deployment_creator: "user".into(),
            deployment_type: "IN_PLACE".into(),
            app_spec_path: "appspec.yml".into(),
            file_exists_behavior: "DISALLOW".into(),
            revision_source: RevisionSource::LocalFile,
            revision: RevisionLocation::Local {
                location: "/tmp/bundle.tar".into(),
                bundle_type: "tar".into(),
            },
            all_possible_lifecycle_events: None,
            reuse_archive_from_deployment_id: None,
        }
    }

    #[test]
    fn process_fails_when_get_spec_fails() {
        let dir = TempDir::new().unwrap();
        let processor = CommandProcessor::new(
            test_client(),
            test_dispatcher(&dir),
            test_tracker(&dir),
            "i-1234".into(),
        );
        let result = processor.process(&test_command());
        // Real client fails with network/HTTP error when service is unreachable
        assert!(result.is_err());
    }

    #[test]
    fn send_acknowledgement_returns_status() {
        let dir = TempDir::new().unwrap();
        let processor = CommandProcessor::new(
            test_client(),
            test_dispatcher(&dir),
            test_tracker(&dir),
            "i-1234".into(),
        );
        // Real client will fail with network error in test environment.
        let result = processor.send_acknowledgement(&test_command(), true);
        assert!(result.is_err());
    }

    #[test]
    fn execute_and_report_unsupported_command() {
        let dir = TempDir::new().unwrap();
        let processor = CommandProcessor::new(
            test_client(),
            test_dispatcher(&dir),
            test_tracker(&dir),
            "i-1234".into(),
        );
        let mut cmd = test_command();
        cmd.command_name = "UnknownCommand".into();
        let result = processor.execute_and_report(&cmd, &test_spec(), Instant::now());
        assert!(result.is_err());
    }

    #[test]
    fn execute_and_report_local_dir_succeeds() {
        let dir = TempDir::new().unwrap();
        let processor = CommandProcessor::new(
            test_client(),
            test_dispatcher(&dir),
            test_tracker(&dir),
            "i-1234".into(),
        );
        let mut cmd = test_command();
        cmd.command_name = "DownloadBundle".into();
        let mut spec = test_spec();
        spec.revision_source = RevisionSource::LocalDirectory;
        spec.revision = RevisionLocation::Local {
            location: dir.path().join("src").display().to_string(),
            bundle_type: "directory".into(),
        };
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/appspec.yml"), "version: 0.0\nos: linux\n").unwrap();

        let result = processor.execute_and_report(&cmd, &spec, Instant::now());
        assert!(result.is_ok());
    }

    #[test]
    fn report_completion_does_not_panic() {
        let dir = TempDir::new().unwrap();
        let processor = CommandProcessor::new(
            test_client(),
            test_dispatcher(&dir),
            test_tracker(&dir),
            "i-1234".into(),
        );
        // Deadline already passed: one attempt each, no retry against the real endpoint.
        processor.report_completion(&test_command(), "Succeeded", "{}", Instant::now());
        processor.report_completion(&test_command(), "Failed", "{}", Instant::now());
    }

    #[test]
    fn deployment_system_mismatch() {
        let dir = TempDir::new().unwrap();
        let mut processor = CommandProcessor::new(
            test_client(),
            test_dispatcher(&dir),
            test_tracker(&dir),
            "i-1234".into(),
        );
        processor.deployment_system = "OtherSystem".into();
        let result = processor.process(&test_command());
        assert!(result.is_err());
    }

    #[test]
    fn parse_spec_valid_json() {
        let json = serde_json::json!({
            "DeploymentId": "d-123",
            "DeploymentGroupId": "dg-1",
            "DeploymentGroupName": "my-group",
            "ApplicationName": "my-app",
            "Revision": {
                "RevisionType": "Local File",
                "LocalRevision": {
                    "Location": "/tmp/bundle.tar",
                    "BundleType": "tar"
                }
            }
        });
        let spec =
            CommandProcessor::<FileBasedDeploymentTracker<SystemFileOperations>>::parse_spec(
                &json.to_string(),
                "TEXT/JSON",
            )
            .unwrap();
        assert_eq!(spec.deployment_id, "d-123");
    }

    #[test]
    fn parse_spec_invalid_json() {
        let result =
            CommandProcessor::<FileBasedDeploymentTracker<SystemFileOperations>>::parse_spec(
                "not json",
                "TEXT/JSON",
            );
        assert!(result.is_err());
    }

    #[test]
    fn send_acknowledgement_not_noop() {
        let dir = TempDir::new().unwrap();
        let processor = CommandProcessor::new(
            test_client(),
            test_dispatcher(&dir),
            test_tracker(&dir),
            "i-1234".into(),
        );
        // Real client will fail with network error in test environment
        let result = processor.send_acknowledgement(&test_command(), false);
        assert!(result.is_err());
    }

    #[test]
    fn execute_and_report_tracks_and_cleans_up() {
        let dir = TempDir::new().unwrap();
        let tracker = test_tracker(&dir);
        let processor =
            CommandProcessor::new(test_client(), test_dispatcher(&dir), tracker, "i-1234".into());

        // Use a noop hook command — will succeed without needing real files
        let cmd = test_command(); // AfterInstall with empty mapping = noop
        let result = processor.execute_and_report(&cmd, &test_spec(), Instant::now());
        // Succeeds (noop hook) or fails (unsupported) — either way tracking is cleaned
        let _ = result;

        // Tracking file should be cleaned up
        assert!(!processor.tracker.is_deployment_in_progress().unwrap());
    }

    // Regression: a malformed AppSpec must not make `is_command_noop`
    // return Err — that would bypass `send_acknowledgement` +
    // `execute_and_report` and strand the deployment as InProgress.
    // `is_noop` now checks the mapping only; parse errors surface later
    // inside `execute_and_report`.
    #[test]
    fn is_command_noop_does_not_error_on_malformed_appspec() {
        let dir = TempDir::new().unwrap();

        // Build a dispatcher whose hook mapping has an AfterInstall event.
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&instructions).unwrap();
        let archives = Arc::new(DeploymentArchives::new(root, instructions, 5));
        let mut mapping = HashMap::new();
        mapping.insert("AfterInstall".to_string(), vec!["AfterInstall".to_string()]);
        let dispatcher = CommandDispatcher::new(
            archives.clone(),
            None,
            mapping,
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );

        // Plant a malformed AppSpec (invalid MLS range) in the archive dir.
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        std::fs::create_dir_all(&archive_dir).unwrap();
        std::fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: invalid\n",
        )
        .unwrap();

        // is_command_noop returns false because the mapping has events.
        assert!(
            !dispatcher.is_command_noop("AfterInstall", &test_spec()),
            "AfterInstall has events in mapping; expected non-noop regardless of AppSpec state"
        );
    }

    // --- Mock-based tests for ack-status branching logic ---

    use std::cell::RefCell;
    use std::collections::VecDeque;

    struct MockClient {
        ack_status: AckStatus,
        /// Every `put_host_command_complete` call, including failed attempts.
        completions: RefCell<Vec<(String, String)>>,
        /// Errors returned by successive completion calls before they succeed.
        complete_errors: RefCell<VecDeque<CodeDeployClientError>>,
        /// When set, every completion call fails with this error kind.
        always_fail: Option<codedeploy_commands::ErrorKind>,
        /// Number of `reset_connections` calls.
        resets: std::cell::Cell<u32>,
    }

    impl MockClient {
        fn new(ack_status: AckStatus) -> Self {
            Self {
                ack_status,
                completions: RefCell::new(Vec::new()),
                complete_errors: RefCell::new(VecDeque::new()),
                always_fail: None,
                resets: std::cell::Cell::new(0),
            }
        }

        fn failing_completions_with(mut self, errors: Vec<CodeDeployClientError>) -> Self {
            self.complete_errors = RefCell::new(errors.into());
            self
        }
    }

    impl std::fmt::Debug for MockClient {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("MockClient").finish()
        }
    }

    impl super::CommandServiceClient for MockClient {
        fn get_deployment_specification(
            &self,
            _deployment_execution_id: &str,
            _host_identifier: &str,
        ) -> Result<
            crate::aws_clients::codedeploy_command_client::DeploymentSpecification,
            crate::aws_clients::codedeploy_command_client::CodeDeployClientError,
        > {
            Ok(crate::aws_clients::codedeploy_command_client::DeploymentSpecification {
                deployment_system: "CodeDeploy".into(),
                generic_envelope: serde_json::json!({
                    "DeploymentId": "d-123",
                    "DeploymentGroupId": "dg-1",
                    "DeploymentGroupName": "my-group",
                    "ApplicationName": "my-app",
                    "Revision": {
                        "RevisionType": "Local File",
                        "LocalRevision": {
                            "Location": "/tmp/bundle.tar",
                            "BundleType": "tar"
                        }
                    }
                })
                .to_string(),
                envelope_format: "TEXT/JSON".into(),
            })
        }

        fn put_host_command_acknowledgement(
            &self,
            _host_command_identifier: &str,
            _diagnostics_payload: &str,
        ) -> Result<AckStatus, crate::aws_clients::codedeploy_command_client::CodeDeployClientError>
        {
            Ok(match self.ack_status {
                AckStatus::InProgress => AckStatus::InProgress,
                AckStatus::Failed => AckStatus::Failed,
                AckStatus::Succeeded => AckStatus::Succeeded,
            })
        }

        fn put_host_command_complete(
            &self,
            _host_command_identifier: &str,
            status: &str,
            diagnostics_payload: &str,
        ) -> Result<(), crate::aws_clients::codedeploy_command_client::CodeDeployClientError>
        {
            self.completions.borrow_mut().push((status.into(), diagnostics_payload.into()));
            if let Some(kind) = &self.always_fail {
                return Err(CodeDeployClientError::Service(
                    codedeploy_commands::Error::new(kind.clone(), "always failing")
                        .with_status(503),
                ));
            }
            match self.complete_errors.borrow_mut().pop_front() {
                Some(e) => Err(e),
                None => Ok(()),
            }
        }

        fn reset_connections(
            &self,
        ) -> Result<(), crate::aws_clients::codedeploy_command_client::CodeDeployClientError>
        {
            self.resets.set(self.resets.get() + 1);
            Ok(())
        }
    }

    fn mock_processor(
        dir: &TempDir,
        ack_status: AckStatus,
    ) -> CommandProcessor<FileBasedDeploymentTracker<SystemFileOperations>, MockClient> {
        CommandProcessor::new(
            MockClient::new(ack_status),
            test_dispatcher(dir),
            test_tracker(dir),
            "i-1234".into(),
        )
    }

    #[test]
    fn ack_in_progress_proceeds_to_execution() {
        let dir = TempDir::new().unwrap();
        let processor = mock_processor(&dir, AckStatus::InProgress);
        let mut cmd = test_command();
        cmd.command_name = "AfterInstall".into();
        let result = processor.process(&cmd);
        // Verify execution proceeded past ack and called put_host_command_complete
        let _ = result;
        let completions = processor.client.completions.borrow();
        assert!(
            !completions.is_empty(),
            "Expected execution to proceed past ack and call put_host_command_complete"
        );
        drop(completions);
        assert!(!processor.tracker.is_deployment_in_progress().unwrap());
    }

    /// A failing hook's completion payload carries the `ScriptError` code
    /// (`ScriptFailed` = 4) and the script's output tail, not a flattened
    /// `from_error` diagnostic.
    #[cfg(unix)]
    #[test]
    fn failing_hook_reports_script_error_code_and_log() {
        use crate::config::AgentConfig;
        use crate::host_command::DeploymentArchives;
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&instructions).unwrap();
        let archives = Arc::new(DeploymentArchives::new(root, instructions, 5));

        // Map AfterInstall to a hook and plant a script that prints a
        // recognizable line to stderr, then exits non-zero.
        let mut mapping = HashMap::new();
        mapping.insert("AfterInstall".to_string(), vec!["AfterInstall".to_string()]);
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        let scripts = archive_dir.join("scripts");
        std::fs::create_dir_all(&scripts).unwrap();
        std::fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\nhooks:\n  AfterInstall:\n    - location: scripts/boom.sh\n      timeout: 10\n",
        )
        .unwrap();
        std::fs::write(scripts.join("boom.sh"), "#!/bin/sh\necho BOOM_MARKER 1>&2\nexit 7\n")
            .unwrap();
        std::fs::set_permissions(scripts.join("boom.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        let dispatcher = CommandDispatcher::new(
            archives,
            None,
            mapping,
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        let processor = CommandProcessor::new(
            MockClient::new(AckStatus::InProgress),
            dispatcher,
            test_tracker(&dir),
            "i-1234".into(),
        );

        let mut cmd = test_command();
        cmd.command_name = "AfterInstall".into();
        let _ = processor.process(&cmd);

        let completions = processor.client.completions.borrow();
        assert_eq!(completions.len(), 1, "expected exactly one completion");
        let (status, payload) = &completions[0];
        assert_eq!(status, "Failed");

        let diag: serde_json::Value = serde_json::from_str(payload).unwrap();
        assert_eq!(
            diag["error_code"], 4,
            "must report ScriptFailed(4), not UnknownError(5); got payload {payload}"
        );
        assert_eq!(diag["script_name"], "scripts/boom.sh");
        assert!(
            diag["log"].as_str().unwrap().contains("BOOM_MARKER"),
            "log tail must carry the script's stderr; got {payload}"
        );
    }

    #[test]
    fn ack_failed_with_noop_reports_completed_noop() {
        let dir = TempDir::new().unwrap();
        let processor = mock_processor(&dir, AckStatus::Failed);
        // Use a command that IS a noop (no mapping for this hook)
        let mut cmd = test_command();
        cmd.command_name = "UnmappedHook".into();
        let result = processor.process(&cmd);
        assert!(result.is_ok());
        let completions = processor.client.completions.borrow();
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].0, "Succeeded");
        assert!(completions[0].1.contains("CompletedNoopCommand"));
    }

    /// A `Failed` acknowledgement must not fail the deployment, and must log the
    /// exact "checking whether command is a noop..." wording that frames it as a
    /// noop check in progress.
    #[tracing_test::traced_test]
    #[test]
    fn failed_ack_logs_noop_wording() {
        let dir = TempDir::new().unwrap();
        let processor = mock_processor(&dir, AckStatus::Failed);
        let mut cmd = test_command();
        cmd.command_name = "DownloadBundle".into(); // non-noop -> early return on Failed ack

        let result = processor.process(&cmd);
        assert!(result.is_ok(), "a Failed ack must not fail the deployment");

        // Expected wording, including the clarifying noop-check tail.
        assert!(
            logs_contain(
                "Received Failed for command DownloadBundle, checking whether command is a noop..."
            ),
            "ack-Failed line should match the expected wording"
        );
    }

    // --- Completion retry on transient errors ---

    /// Millisecond-scale policy so retry tests run fast.
    fn fast_retry_policy(deadline: Duration) -> CompletionRetryPolicy {
        CompletionRetryPolicy {
            deadline_after_ack: deadline,
            initial_backoff: Duration::from_millis(1),
            max_backoff: Duration::from_millis(5),
        }
    }

    fn http_error(status: u16) -> CodeDeployClientError {
        CodeDeployClientError::Service(
            codedeploy_commands::Error::new(codedeploy_commands::ErrorKind::Http, "boom")
                .with_status(status),
        )
    }

    fn kind_error(kind: codedeploy_commands::ErrorKind) -> CodeDeployClientError {
        CodeDeployClientError::Service(codedeploy_commands::Error::new(kind, "boom"))
    }

    fn processor_with(
        dir: &TempDir,
        client: MockClient,
        policy: CompletionRetryPolicy,
    ) -> CommandProcessor<FileBasedDeploymentTracker<SystemFileOperations>, MockClient> {
        CommandProcessor::new(client, test_dispatcher(dir), test_tracker(dir), "i-1234".into())
            .with_completion_retry_policy(policy)
    }

    /// The 10-15 and 10-23 incidents: a burst of 503s / bodyless 500s / resets
    /// on `PutHostCommandComplete`. The result must still land, unchanged.
    #[test]
    fn transient_completion_errors_are_retried_until_success() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(AckStatus::Failed).failing_completions_with(vec![
            http_error(503),
            http_error(500),
            kind_error(codedeploy_commands::ErrorKind::Network),
            kind_error(codedeploy_commands::ErrorKind::ServerException),
        ]);
        let processor = processor_with(&dir, client, fast_retry_policy(Duration::from_secs(10)));
        let mut cmd = test_command();
        cmd.command_name = "UnmappedHook".into(); // noop -> reports Succeeded on Failed ack

        assert!(processor.process(&cmd).is_ok());

        let completions = processor.client.completions.borrow();
        assert_eq!(completions.len(), 5, "4 transient failures then 1 success");
        assert!(
            completions.iter().all(|(status, _)| status == "Succeeded"),
            "every attempt must resend the real result, never downgrade it"
        );
    }

    #[test]
    fn hook_result_is_retried_after_execution() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(AckStatus::InProgress)
            .failing_completions_with(vec![http_error(503), http_error(503)]);
        let processor = processor_with(&dir, client, fast_retry_policy(Duration::from_secs(10)));
        let mut cmd = test_command();
        cmd.command_name = "AfterInstall".into();

        let _ = processor.process(&cmd);

        let completions = processor.client.completions.borrow();
        assert_eq!(completions.len(), 3, "2 transient failures then 1 success");
        let first = &completions[0];
        assert!(completions.iter().all(|c| c == first), "retries resend the same status/payload");
        drop(completions);
        assert!(!processor.tracker.is_deployment_in_progress().unwrap());
    }

    #[test]
    fn client_errors_are_not_retried() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(AckStatus::Failed).failing_completions_with(vec![
            kind_error(codedeploy_commands::ErrorKind::ClientException),
            http_error(503), // must never be reached
        ]);
        let processor = processor_with(&dir, client, fast_retry_policy(Duration::from_secs(10)));
        let mut cmd = test_command();
        cmd.command_name = "UnmappedHook".into();

        assert!(processor.process(&cmd).is_ok());
        assert_eq!(processor.client.completions.borrow().len(), 1);
    }

    #[test]
    fn http_4xx_is_not_retried() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(AckStatus::Failed)
            .failing_completions_with(vec![http_error(400), http_error(503)]);
        let processor = processor_with(&dir, client, fast_retry_policy(Duration::from_secs(10)));
        let mut cmd = test_command();
        cmd.command_name = "UnmappedHook".into();

        assert!(processor.process(&cmd).is_ok());
        assert_eq!(processor.client.completions.borrow().len(), 1);
    }

    /// Retrying stops at the deadline even if the backend never recovers.
    #[test]
    fn retries_stop_at_deadline() {
        let dir = TempDir::new().unwrap();
        let mut client = MockClient::new(AckStatus::Failed);
        client.always_fail = Some(codedeploy_commands::ErrorKind::Http);
        let deadline = Duration::from_millis(200);
        let processor = processor_with(&dir, client, fast_retry_policy(deadline));
        let mut cmd = test_command();
        cmd.command_name = "UnmappedHook".into();

        let start = Instant::now();
        assert!(processor.process(&cmd).is_ok());
        let elapsed = start.elapsed();

        let attempts = processor.client.completions.borrow().len();
        assert!(attempts > 1, "should have retried; got {attempts} attempt(s)");
        assert!(
            elapsed < deadline + Duration::from_secs(2),
            "must give up near the deadline; took {elapsed:?}"
        );
    }

    /// Only complete 5xx responses reconnect: there the pooled connection is
    /// healthy and would carry the retry back to the same backend host.
    /// Network errors and timeouts already get a new connection from the HTTP
    /// client, and throttling is not host-specific.
    #[test]
    fn only_server_error_responses_reset_connections() {
        let dir = TempDir::new().unwrap();
        let client = MockClient::new(AckStatus::Failed).failing_completions_with(vec![
            http_error(503),
            kind_error(codedeploy_commands::ErrorKind::Throttled),
            kind_error(codedeploy_commands::ErrorKind::Network),
            http_error(500),
        ]);
        let processor = processor_with(&dir, client, fast_retry_policy(Duration::from_secs(10)));
        let mut cmd = test_command();
        cmd.command_name = "UnmappedHook".into();

        assert!(processor.process(&cmd).is_ok());
        assert_eq!(processor.client.completions.borrow().len(), 5);
        assert_eq!(processor.client.resets.get(), 2, "only the 503 and the 500 reconnect");
    }

    #[test]
    fn non_retryable_errors_do_not_reset_connections() {
        let dir = TempDir::new().unwrap();
        let client =
            MockClient::new(AckStatus::Failed).failing_completions_with(vec![http_error(400)]);
        let processor = processor_with(&dir, client, fast_retry_policy(Duration::from_secs(10)));
        let mut cmd = test_command();
        cmd.command_name = "UnmappedHook".into();

        assert!(processor.process(&cmd).is_ok());
        assert_eq!(processor.client.resets.get(), 0);
    }

    #[tracing_test::traced_test]
    #[test]
    fn retry_and_recovery_are_logged() {
        let dir = TempDir::new().unwrap();
        let client =
            MockClient::new(AckStatus::Failed).failing_completions_with(vec![http_error(503)]);
        let processor = processor_with(&dir, client, fast_retry_policy(Duration::from_secs(10)));
        let mut cmd = test_command();
        cmd.command_name = "UnmappedHook".into();

        assert!(processor.process(&cmd).is_ok());
        assert!(logs_contain("Transient error reporting command completion, retrying"));
        assert!(logs_contain("Reported command completion after retrying"));
    }

    #[test]
    fn backoff_is_bounded_by_ceiling() {
        let policy = CompletionRetryPolicy {
            deadline_after_ack: Duration::from_mins(65),
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(30),
        };
        for attempt in 1..=20 {
            let ceiling = Duration::from_secs(1)
                .saturating_mul(2u32.saturating_pow(attempt - 1))
                .min(Duration::from_secs(30));
            for _ in 0..20 {
                assert!(policy.backoff(attempt) <= ceiling, "attempt {attempt}");
            }
        }
        // Large attempt counts must not overflow.
        assert!(policy.backoff(u32::MAX) <= Duration::from_secs(30));
    }

    #[test]
    fn default_policy_stays_inside_service_timeout() {
        assert!(CompletionRetryPolicy::default().deadline_after_ack < Duration::from_mins(70));
    }

    #[test]
    fn ack_failed_without_noop_returns_early() {
        let dir = TempDir::new().unwrap();
        let processor = mock_processor(&dir, AckStatus::Failed);
        // Use a command that is NOT a noop (AfterInstall has a mapping in test_dispatcher)
        let mut cmd = test_command();
        cmd.command_name = "DownloadBundle".into();
        let result = processor.process(&cmd);
        assert!(result.is_ok());
        // No completion reported — early return
        let completions = processor.client.completions.borrow();
        assert_eq!(completions.len(), 0);
    }
}
