//! @risk high
//!
//! Thin dispatcher that routes host commands to their handlers.

use super::DeploymentArchives;
use super::commands::hook::HookMapping;
use super::commands::{DownloadCommand, HookCommand, InstallCommand, UpdateAgentCommand};
use crate::aws_clients::S3Client;
use crate::deployment_specification::types::DeploymentSpec;
use std::fs;
use std::io;
use std::sync::Arc;
use tracing::{debug, info};

/// Default ordered lifecycle events.
const DEFAULT_LIFECYCLE_EVENTS: &[&str] = &[
    "BeforeBlockTraffic",
    "AfterBlockTraffic",
    "ApplicationStop",
    "DownloadBundle",
    "Install",
    "BeforeInstall",
    "AfterInstall",
    "ApplicationStart",
    "BeforeAllowTraffic",
    "AfterAllowTraffic",
    "ValidateService",
];

#[derive(Debug)]
pub struct CommandDispatcher {
    download: DownloadCommand,
    install: InstallCommand,
    hook: HookCommand,
    update: UpdateAgentCommand,
}

impl CommandDispatcher {
    #[must_use]
    pub fn new(
        archives: Arc<DeploymentArchives>,
        s3_client: Option<S3Client>,
        hook_mapping: HookMapping,
    ) -> Self {
        let default_keys: Vec<String> =
            DEFAULT_LIFECYCLE_EVENTS.iter().map(|s| (*s).to_string()).collect();
        let mapping_keys: Vec<String> = hook_mapping.keys().cloned().collect();

        Self {
            download: DownloadCommand::new(Arc::clone(&archives), s3_client),
            install: InstallCommand::new(Arc::clone(&archives), mapping_keys, default_keys),
            hook: HookCommand::new(archives, hook_mapping),
            update: UpdateAgentCommand::new(),
        }
    }

    /// Dispatch a command by name.
    ///
    /// # Errors
    /// Returns an error if the command fails or is unknown.
    pub fn execute_command(
        &self,
        command_name: &str,
        spec: &DeploymentSpec,
    ) -> io::Result<Vec<String>> {
        debug!("Command {command_name} dispatching");

        // UpdateDeploymentAgent bypasses the normal deployment spec flow —
        // no appspec, no file installation, no deployment directory.
        if command_name == "UpdateDeploymentAgent" {
            return self.update.execute();
        }

        let deploy_dir = self
            .hook
            .archives()
            .deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
        fs::create_dir_all(&deploy_dir)?;

        match command_name {
            "DownloadBundle" => {
                self.download.execute(spec)?;
                info!(command_name, deployment_id = %spec.deployment_id, "DownloadBundle completed");
                Ok(Vec::new())
            },
            "Install" => {
                self.install.execute(spec)?;
                info!(command_name, deployment_id = %spec.deployment_id, "Install completed");
                Ok(Vec::new())
            },
            name if self.hook.handles(name) => self
                .hook
                .execute(name, spec)
                .map_err(|e| io::Error::other(format!("Hook {name} failed: {e}"))),
            other => Err(io::Error::other(format!("Unsupported command type: {other}"))),
        }
    }

    /// Check if a command is a noop (all lifecycle events have no scripts).
    /// `DownloadBundle` and `Install` are never noops.
    ///
    /// # Errors
    /// Returns an error if executor creation fails.
    pub fn is_command_noop(&self, command_name: &str, spec: &DeploymentSpec) -> io::Result<bool> {
        if command_name == "DownloadBundle"
            || command_name == "Install"
            || command_name == "UpdateDeploymentAgent"
        {
            return Ok(false);
        }
        self.hook
            .is_noop(command_name, spec)
            .map_err(|e| io::Error::other(format!("Noop check failed for {command_name}: {e}")))
    }

    /// Total timeout for all lifecycle event scripts in a command.
    /// Returns `None` if any script has no timeout or the command has no events.
    #[must_use]
    pub fn total_timeout(&self, command_name: &str, spec: &DeploymentSpec) -> Option<u64> {
        if command_name == "UpdateDeploymentAgent" {
            return None;
        }
        self.hook.total_timeout(command_name, spec)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
    use std::collections::HashMap;

    fn test_archives(dir: &tempfile::TempDir) -> Arc<DeploymentArchives> {
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&instructions).unwrap();
        Arc::new(DeploymentArchives::new(root, instructions, 5))
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
    fn unsupported_command_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(test_archives(&dir), None, HashMap::new());
        let err = dispatcher.execute_command("Unknown", &test_spec()).unwrap_err();
        assert!(err.to_string().contains("Unsupported command type: Unknown"));
    }

    #[test]
    fn download_bundle_never_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(test_archives(&dir), None, HashMap::new());
        assert!(!dispatcher.is_command_noop("DownloadBundle", &test_spec()).unwrap());
    }

    #[test]
    fn install_never_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(test_archives(&dir), None, HashMap::new());
        assert!(!dispatcher.is_command_noop("Install", &test_spec()).unwrap());
    }

    #[test]
    fn unknown_hook_is_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(test_archives(&dir), None, HashMap::new());
        assert!(dispatcher.is_command_noop("AfterInstall", &test_spec()).unwrap());
    }

    #[test]
    fn total_timeout_empty_mapping() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(test_archives(&dir), None, HashMap::new());
        assert_eq!(dispatcher.total_timeout("AfterInstall", &test_spec()), None);
    }

    #[test]
    fn creates_deployment_root_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let mut mapping = HashMap::new();
        mapping.insert("AfterInstall".into(), vec!["AfterInstall".into()]);
        let dispatcher = CommandDispatcher::new(archives.clone(), None, mapping);

        // Execute a hook command — should create deploy dir even if noop
        let _ = dispatcher.execute_command("AfterInstall", &test_spec());
        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        assert!(deploy_dir.exists());
    }

    #[test]
    fn install_command_dispatches() {
        let dir = tempfile::TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let dispatcher = CommandDispatcher::new(archives.clone(), None, HashMap::new());

        // Create archive dir with a minimal appspec
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("appspec.yml"), "version: 0.0\nos: linux\n").unwrap();

        let result = dispatcher.execute_command("Install", &test_spec());
        assert!(result.is_ok());
    }

    #[test]
    fn update_deployment_agent_returns_ok() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(test_archives(&dir), None, HashMap::new());
        let result = dispatcher.execute_command("UpdateDeploymentAgent", &test_spec());
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn update_deployment_agent_never_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(test_archives(&dir), None, HashMap::new());
        assert!(!dispatcher.is_command_noop("UpdateDeploymentAgent", &test_spec()).unwrap());
    }

    #[test]
    fn update_deployment_agent_has_no_timeout() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(test_archives(&dir), None, HashMap::new());
        assert_eq!(dispatcher.total_timeout("UpdateDeploymentAgent", &test_spec()), None);
    }

    #[test]
    fn update_deployment_agent_skips_deploy_dir_creation() {
        let dir = tempfile::TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let dispatcher = CommandDispatcher::new(archives.clone(), None, HashMap::new());
        dispatcher.execute_command("UpdateDeploymentAgent", &test_spec()).unwrap();
        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        assert!(!deploy_dir.exists());
    }
}
