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
use std::time::Instant;
use tracing::{debug, error, info, info_span};

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
}

#[derive(Debug)]
pub struct CommandProcessor<T: DeploymentTracker, C: CommandServiceClient = CodeDeployCommandClient>
{
    client: C,
    dispatcher: CommandDispatcher,
    tracker: T,
    host_identifier: String,
    deployment_system: String,
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
        }
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
                );
                return Ok(());
            },
            _ => return Ok(()),
        }

        self.execute_and_report(command, &spec)
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

    fn execute_and_report(&self, command: &HostCommand, spec: &DeploymentSpec) -> io::Result<()> {
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
                self.report_completion(command, "Succeeded", &diagnostics::success(&note));
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
                self.report_completion(command, "Failed", &payload);
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

    fn report_completion(&self, command: &HostCommand, status: &str, payload: &str) {
        debug!("Calling PutHostCommandComplete: \"{status}\"");
        if let Err(e) =
            self.client
                .put_host_command_complete(&command.host_command_identifier, status, payload)
        {
            error!("Failed to report command completion: {e}");
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
        let result = processor.execute_and_report(&cmd, &test_spec());
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

        let result = processor.execute_and_report(&cmd, &spec);
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
        processor.report_completion(&test_command(), "Succeeded", "{}");
        processor.report_completion(&test_command(), "Failed", "{}");
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
        let result = processor.execute_and_report(&cmd, &test_spec());
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

    struct MockClient {
        ack_status: AckStatus,
        completions: RefCell<Vec<(String, String)>>,
    }

    impl MockClient {
        fn new(ack_status: AckStatus) -> Self {
            Self { ack_status, completions: RefCell::new(Vec::new()) }
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
