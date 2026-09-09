//! Thin dispatcher that routes host commands to their handlers.

use super::DeploymentArchives;
use super::commands::hook::HookMapping;
use super::commands::{DownloadCommand, HookCommand, InstallCommand, UpdateAgentCommand};
use crate::aws_clients::S3Client;
use crate::config::AgentConfig;
use crate::deployment_specification::types::DeploymentSpec;
use crate::logging::LogConfig;
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
    restrict_dir_permissions: bool,
}

impl CommandDispatcher {
    #[must_use]
    pub fn new(
        archives: Arc<DeploymentArchives>,
        s3_client: Option<S3Client>,
        hook_mapping: HookMapping,
        region: &str,
        update_s3_client: Option<S3Client>,
        config: Arc<AgentConfig>,
    ) -> Self {
        let default_keys: Vec<String> =
            DEFAULT_LIFECYCLE_EVENTS.iter().map(|s| (*s).to_string()).collect();
        let mapping_keys: Vec<String> = hook_mapping.keys().cloned().collect();

        // Build the hook command, enabling the per-deployment log when
        // configured. Read config fields before `config` is moved into
        // InstallCommand below.
        let mut hook = HookCommand::new(Arc::clone(&archives), hook_mapping)
            .with_env_policy(crate::lifecycle_event::HookEnvPolicy {
                strip_loader_vars: config.hardening.strip_loader_env_in_hooks,
                restrict_to_allowlist: config.hardening.restrict_hook_env_to_allowlist,
                disable_powershell_profile: config.hardening.disable_powershell_profile_in_hooks,
            })
            .with_reject_path_traversal(config.hardening.reject_path_traversal_in_bundle)
            .with_restrict_log_permissions(config.hardening.restrict_agent_dir_permissions);
        if config.enable_deployments_log {
            hook = hook.with_deployment_log_config(LogConfig {
                log_dir: config.log_dir.clone(),
                verbose: config.verbose,
                program_name: config.program_name.clone(),
                root_dir: config.root_dir.clone(),
                restrict_permissions: config.hardening.restrict_agent_dir_permissions,
                restrict_log_permissions: config.hardening.restrict_log_dir_permissions,
            });
        }

        let restrict_dir_permissions = config.hardening.restrict_agent_dir_permissions;
        let restrict_log_dir_permissions = config.hardening.restrict_log_dir_permissions;
        Self {
            download: DownloadCommand::new(Arc::clone(&archives), s3_client, Arc::clone(&config)),
            // Last use of `archives` — move it rather than clone to consume the
            // by-value argument (satisfies clippy::needless_pass_by_value).
            install: InstallCommand::new(archives, mapping_keys, default_keys, config),
            hook,
            update: UpdateAgentCommand::new(region.to_string(), update_s3_client)
                .with_restrict_log_permissions(restrict_log_dir_permissions),
            restrict_dir_permissions,
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
        // Default: world-readable 0755, because host tooling outside the agent
        // reads these directories.
        // With `restrict_agent_dir_permissions`: 0711 on `<root>/<group>`
        // and `<root>/<group>/<deploy>` — non-root `runas:` users can traverse
        // to the leaf `deployment-archive/` (also 0711) but cannot list
        // contents to enumerate group/deployment IDs.
        if let Some(group_dir) = deploy_dir.parent() {
            crate::system::create_deployment_dir(group_dir, 0o711, self.restrict_dir_permissions)?;
        }
        crate::system::create_deployment_dir(&deploy_dir, 0o711, self.restrict_dir_permissions)?;

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
                // Box the `ScriptError` as the io::Error source (not a string) so
                // the reporter can recover it via `downcast_ref::<ScriptError>()`.
                .map_err(io::Error::other),
            other => Err(io::Error::other(format!("Unsupported command type: {other}"))),
        }
    }

    /// The deployment archives these commands read and write.
    #[must_use]
    pub fn archives(&self) -> &DeploymentArchives {
        self.hook.archives()
    }

    /// Check if a command is a noop (all lifecycle events have no scripts).
    /// `DownloadBundle` and `Install` are never noops.
    #[must_use]
    pub fn is_command_noop(&self, command_name: &str, spec: &DeploymentSpec) -> bool {
        if command_name == "DownloadBundle"
            || command_name == "Install"
            || command_name == "UpdateDeploymentAgent"
        {
            return false;
        }
        self.hook.is_noop(command_name, spec)
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
    use std::fs;

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
            reuse_archive_from_deployment_id: None,
        }
    }

    #[test]
    fn unsupported_command_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        let err = dispatcher.execute_command("Unknown", &test_spec()).unwrap_err();
        assert!(err.to_string().contains("Unsupported command type: Unknown"));
    }

    #[test]
    fn builds_with_deployments_log_disabled() {
        // Exercises the `enable_deployments_log = false` arm of `new`, where the
        // hook command is built without a deployment-log config.
        let dir = tempfile::TempDir::new().unwrap();
        let config = AgentConfig { enable_deployments_log: false, ..AgentConfig::default() };
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(config),
        );
        // Just proves construction succeeds on the disabled path.
        assert!(dispatcher.is_command_noop("AfterInstall", &test_spec()));
    }

    #[test]
    fn download_bundle_never_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        assert!(!dispatcher.is_command_noop("DownloadBundle", &test_spec()));
    }

    #[test]
    fn install_never_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        assert!(!dispatcher.is_command_noop("Install", &test_spec()));
    }

    #[test]
    fn unknown_hook_is_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        assert!(dispatcher.is_command_noop("AfterInstall", &test_spec()));
    }

    #[test]
    fn total_timeout_empty_mapping() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        assert_eq!(dispatcher.total_timeout("AfterInstall", &test_spec()), None);
    }

    #[test]
    fn creates_deployment_root_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let mut mapping = HashMap::new();
        mapping.insert("AfterInstall".into(), vec!["AfterInstall".into()]);
        let dispatcher = CommandDispatcher::new(
            archives.clone(),
            None,
            mapping,
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );

        // Execute a hook command — should create deploy dir even if noop
        let _ = dispatcher.execute_command("AfterInstall", &test_spec());

        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        assert!(deploy_dir.exists());

        // Default: `<root>/<group>` and `<root>/<group>/<deploy>` are
        // world-readable 0755, because host tooling outside the agent reads
        // these directories.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let group_dir = deploy_dir.parent().unwrap();
            let group_mode = std::fs::metadata(group_dir).unwrap().permissions().mode() & 0o777;
            let deploy_mode = std::fs::metadata(&deploy_dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(group_mode, 0o755, "group dir mode {group_mode:o}, expected 0755");
            assert_eq!(deploy_mode, 0o755, "deploy dir mode {deploy_mode:o}, expected 0755");
        }
    }

    #[test]
    fn creates_deployment_root_dir_restricted() {
        let dir = tempfile::TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let mut mapping = HashMap::new();
        mapping.insert("AfterInstall".into(), vec!["AfterInstall".into()]);
        let config = AgentConfig {
            hardening: crate::config::HardeningConfig {
                restrict_agent_dir_permissions: true,
                ..Default::default()
            },
            ..AgentConfig::default()
        };
        let dispatcher = CommandDispatcher::new(
            archives.clone(),
            None,
            mapping,
            "us-east-1",
            None,
            Arc::new(config),
        );

        let _ = dispatcher.execute_command("AfterInstall", &test_spec());

        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        assert!(deploy_dir.exists());

        // Opt-in hardening: 0711 — non-root `runas:` users can traverse to
        // the leaf, but cannot list contents to enumerate group/deployment IDs.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let group_dir = deploy_dir.parent().unwrap();
            let group_mode = std::fs::metadata(group_dir).unwrap().permissions().mode() & 0o777;
            let deploy_mode = std::fs::metadata(&deploy_dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(group_mode, 0o711, "group dir mode {group_mode:o}, expected 0711");
            assert_eq!(deploy_mode, 0o711, "deploy dir mode {deploy_mode:o}, expected 0711");
        }
    }

    #[test]
    fn install_command_dispatches() {
        let dir = tempfile::TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let dispatcher = CommandDispatcher::new(
            archives.clone(),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );

        // Create archive dir with a minimal appspec
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("appspec.yml"), "version: 0.0\nos: linux\n").unwrap();

        let result = dispatcher.execute_command("Install", &test_spec());
        assert!(result.is_ok());
    }

    #[test]
    fn update_deployment_agent_returns_error_without_s3() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        let result = dispatcher.execute_command("UpdateDeploymentAgent", &test_spec());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("S3 client not configured"));
    }

    #[test]
    fn update_deployment_agent_never_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        assert!(!dispatcher.is_command_noop("UpdateDeploymentAgent", &test_spec()));
    }

    #[test]
    fn update_deployment_agent_has_no_timeout() {
        let dir = tempfile::TempDir::new().unwrap();
        let dispatcher = CommandDispatcher::new(
            test_archives(&dir),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        assert_eq!(dispatcher.total_timeout("UpdateDeploymentAgent", &test_spec()), None);
    }

    #[test]
    fn update_deployment_agent_skips_deploy_dir_creation() {
        let dir = tempfile::TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let dispatcher = CommandDispatcher::new(
            archives.clone(),
            None,
            HashMap::new(),
            "us-east-1",
            None,
            Arc::new(AgentConfig::default()),
        );
        // Update now returns error (no S3 client), but deploy dir should still not be created
        let _ = dispatcher.execute_command("UpdateDeploymentAgent", &test_spec());
        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        assert!(!deploy_dir.exists());
    }
}
