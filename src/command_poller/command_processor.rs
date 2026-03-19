//! @risk high
//!
//! Processes a single host command end-to-end.
//!
//! Orchestrates the full lifecycle of a deployment command: fetch the deployment
//! specification, acknowledge receipt, execute the command (download/install/hook),
//! and report the result back to the `CodeDeploy` service.

use super::diagnostics;
use crate::aws_clients::codedeploy_command_client::{
    AckStatus, CodeDeployCommandClient, HostCommand,
};
use crate::deployment_specification::{self, DeploymentSpec};
use crate::host_command::CommandDispatcher;
use crate::runtime::deployment_tracker::DeploymentTracker;
use std::io;
use std::time::Instant;
use tracing::{debug, error, info, info_span};

#[derive(Debug)]
pub struct CommandProcessor<T: DeploymentTracker> {
    client: CodeDeployCommandClient,
    dispatcher: CommandDispatcher,
    tracker: T,
    host_identifier: String,
    deployment_system: String,
}

impl<T: DeploymentTracker> CommandProcessor<T> {
    #[must_use]
    pub fn new(
        client: CodeDeployCommandClient,
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

        let is_noop = self.dispatcher.is_command_noop(&command.command_name, &spec)?;
        let status = self.send_acknowledgement(command, is_noop)?;

        // @risk high — wrong match here skips execution or double-executes
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

        if matches!(status, AckStatus::Failed) {
            info!("Received Failed for command {}", command.command_name);
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

        // @risk high — wrong status string silently misreports deployment outcome
        match &result {
            Ok(_) => {
                info!(
                    elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX),
                    "Command succeeded"
                );
                self.report_completion(command, "Succeeded", &diagnostics::success(""));
            },
            Err(e) => {
                error!(
                    elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX),
                    error = %e,
                    "Command failed"
                );
                self.report_completion(command, "Failed", &diagnostics::from_error(e));
            },
        }

        if let Err(e) = self.tracker.stop_tracking(&spec.deployment_id) {
            error!("Failed to stop deployment tracking: {e}");
        }

        result.map(|_| ())
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
        CommandDispatcher::new(archives, None, HashMap::new())
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
        // TODO: introduce a trait or mock for CodeDeployCommandClient so we can
        // test the ack-status branching logic (InProgress vs Failed+noop) without
        // a live service. The AckStatus::from_response mapping is now unit-tested
        // in codedeploy_command_client.rs.
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
}
