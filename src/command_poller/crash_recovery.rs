//! Crash recovery — detect and fail in-progress deployments after agent restart.
//!
//! On startup, checks if a deployment was interrupted by an agent crash or restart.
//! If found, reports it as failed to the `CodeDeploy` service and cleans up tracking state.

use super::diagnostics;
use crate::aws_clients::CodeDeployCommandClient;
use crate::runtime::deployment_tracker::DeploymentTracker;
use tracing::{error, info, warn};

/// Check if any deployments were in progress when the agent last stopped.
/// If so, report each as failed and clean up the tracking files.
///
/// Loops to handle multiple interrupted deployments (e.g. agent crashed,
/// restarted, crashed again before recovery completed).
///
/// Returns `true` if at least one stale deployment was found and reported.
pub fn recover<T: DeploymentTracker>(client: &CodeDeployCommandClient, tracker: &T) -> bool {
    let mut recovered_any = false;

    loop {
        match tracker.get_active_deployment() {
            Ok(Some(active)) => {
                warn!(
                    "Deployment tracking file found for {}. \
                     The agent likely restarted while running a customer-supplied script. \
                     Failing the lifecycle event.",
                    active.deployment_id
                );

                let payload = diagnostics::from_failure_after_restart(
                    "Failing in-progress lifecycle event after an agent restart.",
                );

                info!(
                    "Calling PutHostCommandComplete: 'Failed' {}",
                    active.host_command_identifier
                );

                // If put_host_command_complete fails, break without removing the tracking
                // file so it survives for retry on next startup.
                if let Err(e) = client.put_host_command_complete(
                    &active.host_command_identifier,
                    "Failed",
                    &payload,
                ) {
                    error!("Failed to report crash recovery for {}: {e}", active.deployment_id);
                    break;
                }

                // Remove this single tracking file so the next iteration picks up the next one.
                if let Err(e) = tracker.stop_tracking(&active.deployment_id) {
                    error!("Failed to clean deployment tracking for {}: {e}", active.deployment_id);
                    // If we can't remove the file, clean_all to avoid infinite loop.
                    if let Err(e2) = tracker.clean_all() {
                        error!("Failed to clean all deployment tracking: {e2}");
                    }
                    break;
                }

                recovered_any = true;
            },
            Ok(None) => break,
            Err(e) => {
                error!("Exception thrown during restart recovery: {e}");
                break;
            },
        }
    }

    recovered_any
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aws_clients::{CredentialMode, Credentials};
    use crate::runtime::file_based_deployment_tracker::FileBasedDeploymentTracker;
    use crate::system::SystemFileOperations;
    use tempfile::TempDir;

    fn test_client() -> CodeDeployCommandClient {
        let creds = Credentials {
            region: "us-east-1".into(),
            host_identifier: "test".into(),
            mode: CredentialMode::IamUser {
                access_key_id: "AKIATEST".into(),
                secret_access_key: "test-secret".into(),
            },
        };
        CodeDeployCommandClient::new(
            creds,
            false,
            false,
            None,
            std::time::Duration::from_secs(80),
            None,
        )
        .unwrap()
    }

    #[test]
    fn no_active_deployment_returns_false() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());
        assert!(!recover(&test_client(), &tracker));
    }

    #[test]
    fn active_deployment_preserved_when_service_unreachable() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());
        tracker.start_tracking("d-123", "cmd-456").unwrap();

        // test_client() points at no real endpoint, so put_host_command_complete fails.
        // recover() breaks without deleting the tracking file (no silent loss).
        let result = recover(&test_client(), &tracker);

        assert!(!result);
        assert!(
            tracker.is_deployment_in_progress().unwrap(),
            "tracking file should be preserved when service call fails"
        );
    }

    #[test]
    fn multiple_deployments_preserved_when_service_unreachable() {
        let dir = TempDir::new().unwrap();
        let tracker =
            FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());
        tracker.start_tracking("d-1", "cmd-1").unwrap();
        tracker.start_tracking("d-2", "cmd-2").unwrap();
        tracker.start_tracking("d-3", "cmd-3").unwrap();

        // Use explicit timestamps to avoid sleep-based flakiness (matches tracker test pattern)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        #[allow(clippy::cast_possible_wrap)]
        let now = now as i64;
        filetime::set_file_mtime(
            dir.path().join("d-1"),
            filetime::FileTime::from_unix_time(now - 20, 0),
        )
        .unwrap();
        filetime::set_file_mtime(
            dir.path().join("d-2"),
            filetime::FileTime::from_unix_time(now - 10, 0),
        )
        .unwrap();
        filetime::set_file_mtime(
            dir.path().join("d-3"),
            filetime::FileTime::from_unix_time(now, 0),
        )
        .unwrap();

        // Service unreachable → breaks on first deployment, all 3 files preserved.
        let result = recover(&test_client(), &tracker);

        assert!(!result);
        // All three tracking files should still exist
        assert!(dir.path().join("d-1").exists());
        assert!(dir.path().join("d-2").exists());
        assert!(dir.path().join("d-3").exists());
    }

    struct FailingTracker;
    impl DeploymentTracker for FailingTracker {
        fn start_tracking(
            &self,
            _: &str,
            _: &str,
        ) -> Result<(), crate::runtime::DeploymentTrackerError> {
            unimplemented!()
        }
        fn stop_tracking(&self, _: &str) -> Result<(), crate::runtime::DeploymentTrackerError> {
            unimplemented!()
        }
        fn get_active_deployment(
            &self,
        ) -> Result<Option<crate::runtime::ActiveDeployment>, crate::runtime::DeploymentTrackerError>
        {
            Err(crate::runtime::DeploymentTrackerError::Io(std::io::Error::other("disk error")))
        }
        fn is_deployment_in_progress(
            &self,
        ) -> Result<bool, crate::runtime::DeploymentTrackerError> {
            unimplemented!()
        }
        fn clean_all(&self) -> Result<(), crate::runtime::DeploymentTrackerError> {
            unimplemented!()
        }
    }

    #[test]
    fn tracker_error_returns_false() {
        assert!(!recover(&test_client(), &FailingTracker));
    }
}
