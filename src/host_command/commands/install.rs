//! Install command.
//!
//! Resolves and validates the appspec, creates the Installer, runs it,
//! and records the last successful install.

use crate::application_specification::{AppSpec, FileExistsBehavior};
use crate::config::AgentConfig;
use crate::deployment_specification::types::DeploymentSpec;
use crate::host_command::DeploymentArchives;
use crate::host_command::appspec_validator;
use crate::installer::Installer;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Debug)]
pub struct InstallCommand {
    archives: Arc<DeploymentArchives>,
    hook_mapping_keys: Vec<String>,
    default_mapping_keys: Vec<String>,
    config: Arc<AgentConfig>,
}

impl InstallCommand {
    #[must_use]
    pub fn new(
        archives: Arc<DeploymentArchives>,
        hook_mapping_keys: Vec<String>,
        default_mapping_keys: Vec<String>,
        config: Arc<AgentConfig>,
    ) -> Self {
        Self { archives, hook_mapping_keys, default_mapping_keys, config }
    }

    /// Execute the `Install` command.
    ///
    /// # Errors
    /// Returns an error if appspec validation, installation, or archive tracking fails.
    pub fn execute(&self, spec: &DeploymentSpec) -> io::Result<()> {
        let deploy_dir = self
            .archives
            .deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
        let archive_dir = self.archives.archive_dir(&spec.deployment_group_id, &spec.deployment_id);
        let instructions_dir = self.archives.instructions_dir();

        fs::create_dir_all(instructions_dir)?;
        debug!("Instructions directory created at {}", instructions_dir.display());

        let app_spec = self.validated_appspec(&archive_dir, spec)?;

        let file_exists_behavior = FileExistsBehavior::parse(&spec.file_exists_behavior)
            .map_err(|_| {
                io::Error::other(format!(
                    "The deployment failed because an invalid option was specified for \
                     fileExistsBehavior: {}. Valid options include OVERWRITE, RETAIN, and DISALLOW.",
                    spec.file_exists_behavior
                ))
            })?;

        let installer = Installer::with_options(
            archive_dir,
            instructions_dir.to_path_buf(),
            file_exists_behavior,
            self.config.hardening.reject_unconfined_selinux_in_bundle,
            self.config.hardening.reject_unsafe_permissions_in_bundle,
            self.config.hardening.reject_path_traversal_in_bundle,
        )
        .with_restrict_permissions(self.config.hardening.restrict_agent_dir_permissions)
        .with_reject_symlink_permission_targets(
            self.config.hardening.reject_symlink_permission_targets,
        );

        debug!("Installing revision in instance group {}", spec.deployment_group_id);

        installer.install(&spec.deployment_group_id, &app_spec).map_err(|e| {
            io::Error::other(format!("Install failed for group {}: {e}", spec.deployment_group_id))
        })?;

        // GRCOV_STOP_COVERAGE
        info!(
            deployment_id = %spec.deployment_id,
            deployment_group = %spec.deployment_group_id,
            file_exists_behavior = %spec.file_exists_behavior,
            "Install completed");
        // GRCOV_BEGIN_COVERAGE

        self.archives.update_last_successful(&spec.deployment_group_id, &deploy_dir)?;

        Ok(())
    }

    fn validated_appspec(&self, archive_dir: &Path, spec: &DeploymentSpec) -> io::Result<AppSpec> {
        let app_spec_path = resolve_appspec_path(archive_dir, &spec.app_spec_path);
        let app_spec = AppSpec::from_file(&app_spec_path).map_err(|e| {
            io::Error::other(format!("Failed to parse appspec at {}: {e}", app_spec_path.display()))
        })?;

        let filename = app_spec_path.file_name().and_then(|n| n.to_str()).unwrap_or("appspec.yml");

        appspec_validator::validate_hooks(
            &app_spec,
            filename,
            spec.all_possible_lifecycle_events.as_deref(),
            &self.hook_mapping_keys,
            &self.default_mapping_keys,
        )
        .map_err(io::Error::other)?;

        Ok(app_spec)
    }
}

/// Resolve appspec path: check param path, then .yaml, then .yml.
fn resolve_appspec_path(archive_dir: &Path, app_spec_path: &str) -> PathBuf {
    let param_path = archive_dir.join(app_spec_path);
    if param_path.exists() {
        debug!("Using appspec file {}", param_path.display());
        return param_path;
    }

    let yaml_path = archive_dir.join("appspec.yaml");
    if yaml_path.exists() {
        debug!("Using appspec file {}", yaml_path.display());
        return yaml_path;
    }

    let yml_path = archive_dir.join("appspec.yml");
    debug!("Using appspec file {}", yml_path.display());
    yml_path
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
    use tempfile::TempDir;

    fn test_archives(dir: &TempDir) -> Arc<DeploymentArchives> {
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

    fn test_cmd(archives: Arc<DeploymentArchives>) -> InstallCommand {
        InstallCommand::new(archives, Vec::new(), Vec::new(), Arc::new(AgentConfig::default()))
    }

    #[test]
    fn install_creates_instructions_dir_and_updates_last_successful() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = test_cmd(archives.clone());

        let spec = test_spec();
        let dest = dir.path().join("output");
        fs::create_dir_all(&dest).unwrap();

        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::create_dir_all(archive_dir.join("src")).unwrap();
        fs::write(archive_dir.join("src/hello.txt"), "world").unwrap();
        fs::write(
            archive_dir.join("appspec.yml"),
            format!(
                "version: 0.0\nos: linux\nfiles:\n  - source: src\n    destination: {}\n",
                dest.display()
            ),
        )
        .unwrap();

        cmd.execute(&spec).unwrap();

        assert!(archives.last_successful_dir("dg-1").is_some());
        assert!(archives.instructions_dir().exists());
        assert!(dest.join("hello.txt").exists());
    }

    #[test]
    fn install_invalid_file_exists_behavior_errors() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = test_cmd(archives.clone());

        let mut spec = test_spec();
        spec.file_exists_behavior = "INVALID".into();

        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("appspec.yml"), "version: 0.0\nos: linux\n").unwrap();

        let result = cmd.execute(&spec);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid option"));
    }

    #[test]
    fn resolve_appspec_prefers_param_path() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("archive");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("custom.yml"), "version: 0.0").unwrap();
        fs::write(archive.join("appspec.yaml"), "version: 0.0").unwrap();

        let result = resolve_appspec_path(&archive, "custom.yml");
        assert!(result.ends_with("custom.yml"));
    }

    #[test]
    fn resolve_appspec_falls_back_to_yaml() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("archive");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("appspec.yaml"), "version: 0.0").unwrap();

        let result = resolve_appspec_path(&archive, "appspec.yml");
        assert!(result.ends_with("appspec.yaml"));
    }

    #[test]
    fn resolve_appspec_defaults_to_yml() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("archive");
        fs::create_dir_all(&archive).unwrap();

        let result = resolve_appspec_path(&archive, "appspec.yml");
        assert!(result.ends_with("appspec.yml"));
    }

    #[test]
    fn install_fails_when_appspec_parse_fails() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = test_cmd(archives.clone());

        let spec = test_spec();
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("appspec.yml"), "invalid: yaml: content:").unwrap();

        let result = cmd.execute(&spec);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to parse appspec"));
    }

    #[test]
    fn install_fails_when_installer_fails() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = test_cmd(archives.clone());

        let spec = test_spec();
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\nfiles:\n  - source: nonexistent\n    destination: /tmp/dest\n",
        )
        .unwrap();

        let result = cmd.execute(&spec);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Install failed for group"));
    }
}
