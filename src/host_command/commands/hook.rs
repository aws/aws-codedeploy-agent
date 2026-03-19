//! @risk medium
//!
//! Hook command — runs lifecycle event scripts.
//!
//! Maps command names to lists of lifecycle event names, then creates and
//! executes a `LifecycleEventExecutor` for each event in sequence.

use crate::deployment_specification::types::DeploymentSpec;
use crate::host_command::DeploymentArchives;
use crate::lifecycle_event::{LifecycleEventExecutor, ScriptError};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::info;

/// Maps command names to lists of lifecycle event names.
pub type HookMapping = HashMap<String, Vec<String>>;

#[derive(Debug)]
pub struct HookCommand {
    archives: Arc<DeploymentArchives>,
    hook_mapping: HookMapping,
}

impl HookCommand {
    #[must_use]
    pub fn new(archives: Arc<DeploymentArchives>, hook_mapping: HookMapping) -> Self {
        Self { archives, hook_mapping }
    }

    /// Whether this command name is in the hook mapping.
    #[must_use]
    pub fn handles(&self, command_name: &str) -> bool {
        self.hook_mapping.contains_key(command_name)
    }

    /// Access to the shared archives.
    #[must_use]
    pub fn archives(&self) -> &DeploymentArchives {
        &self.archives
    }

    /// All command names in the hook mapping.
    #[must_use]
    pub fn mapping_keys(&self) -> Vec<String> {
        self.hook_mapping.keys().cloned().collect()
    }

    /// Execute all lifecycle events for a hook command.
    ///
    /// # Errors
    /// Returns a `ScriptError` if any lifecycle event script fails.
    pub fn execute(
        &self,
        command_name: &str,
        spec: &DeploymentSpec,
    ) -> Result<Vec<String>, ScriptError> {
        let Some(events) = self.hook_mapping.get(command_name) else {
            return Ok(Vec::new());
        };

        info!(
            command_name,
            events = ?events,
            deployment_id = %spec.deployment_id,
            "Executing hook command"
        );

        let mut all_logs = Vec::new();
        for event_name in events {
            let executor = self.create_executor(event_name, spec)?;
            let logs = executor.execute()?;
            all_logs.extend(logs);
        }
        Ok(all_logs)
    }

    /// Check if all lifecycle events for a command are noops.
    ///
    /// # Errors
    /// Returns an error if executor creation fails.
    pub fn is_noop(&self, command_name: &str, spec: &DeploymentSpec) -> Result<bool, ScriptError> {
        let Some(events) = self.hook_mapping.get(command_name) else {
            return Ok(true);
        };

        for event_name in events {
            let executor = self.create_executor(event_name, spec)?;
            if !executor.is_noop() {
                return Ok(false);
            }
            info!("Lifecycle event {event_name} is a noop");
        }

        info!("Noop check completed for command {command_name}, all lifecycle events are noops.");
        Ok(true)
    }

    /// Sum of all script timeouts across lifecycle events for a command.
    /// Returns `None` if any event has no timeout specified.
    #[must_use]
    pub fn total_timeout(&self, command_name: &str, spec: &DeploymentSpec) -> Option<u64> {
        let empty = Vec::new();
        let events = self.hook_mapping.get(command_name).unwrap_or(&empty);
        if events.is_empty() {
            info!("Command {command_name} has no script timeouts specified in appspec.");
            return None;
        }

        let mut total: u64 = 0;
        for event_name in events {
            let executor = self.create_executor(event_name, spec).ok()?;
            // Every script always has a timeout (default 3600), so total_timeout()
            // returns None only when is_noop() — which means no scripts exist.
            total += executor.total_timeout()?;
        }

        info!("Command {command_name} has total script timeout {total} in appspec.");
        Some(total)
    }

    fn create_executor(
        &self,
        event_name: &str,
        spec: &DeploymentSpec,
    ) -> Result<LifecycleEventExecutor, ScriptError> {
        let deploy_dir = self
            .archives
            .deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
        let last_successful = self.archives.last_successful_dir(&spec.deployment_group_id);
        let most_recent = self.archives.most_recent_dir(&spec.deployment_group_id);

        let lifecycle_event = event_name.parse().map_err(|e: String| {
            ScriptError::new(
                crate::lifecycle_event::ErrorCode::UnknownError,
                event_name.to_string(),
                Vec::new(),
                e,
            )
        })?;

        LifecycleEventExecutor::new(
            lifecycle_event,
            spec,
            &deploy_dir,
            last_successful.as_deref(),
            most_recent.as_deref(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
    use std::fs;
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

    fn test_mapping() -> HookMapping {
        let mut m = HashMap::new();
        m.insert("AfterInstall".into(), vec!["AfterInstall".into()]);
        m.insert("BeforeInstall".into(), vec!["BeforeInstall".into()]);
        m
    }

    #[test]
    fn handles_known_command() {
        let dir = TempDir::new().unwrap();
        let cmd = HookCommand::new(test_archives(&dir), test_mapping());
        assert!(cmd.handles("AfterInstall"));
        assert!(!cmd.handles("Unknown"));
    }

    #[test]
    fn mapping_keys_returns_all() {
        let dir = TempDir::new().unwrap();
        let cmd = HookCommand::new(test_archives(&dir), test_mapping());
        let mut keys = cmd.mapping_keys();
        keys.sort();
        assert_eq!(keys, vec!["AfterInstall", "BeforeInstall"]);
    }

    #[test]
    fn execute_unknown_command_returns_empty() {
        let dir = TempDir::new().unwrap();
        let cmd = HookCommand::new(test_archives(&dir), test_mapping());
        let result = cmd.execute("Unknown", &test_spec()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn is_noop_unknown_command() {
        let dir = TempDir::new().unwrap();
        let cmd = HookCommand::new(test_archives(&dir), test_mapping());
        assert!(cmd.is_noop("Unknown", &test_spec()).unwrap());
    }

    #[test]
    fn is_noop_no_archive_dir() {
        let dir = TempDir::new().unwrap();
        let cmd = HookCommand::new(test_archives(&dir), test_mapping());
        // No archive dir exists, so executor finds no appspec -> noop
        assert!(cmd.is_noop("AfterInstall", &test_spec()).unwrap());
    }

    #[test]
    fn total_timeout_empty_mapping() {
        let dir = TempDir::new().unwrap();
        let cmd = HookCommand::new(test_archives(&dir), HashMap::new());
        assert_eq!(cmd.total_timeout("AfterInstall", &test_spec()), None);
    }

    #[test]
    fn total_timeout_no_archive() {
        let dir = TempDir::new().unwrap();
        let cmd = HookCommand::new(test_archives(&dir), test_mapping());
        // No archive -> executor has no appspec -> noop -> None timeout
        assert_eq!(cmd.total_timeout("AfterInstall", &test_spec()), None);
    }

    #[test]
    fn execute_with_appspec_noop_event() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = HookCommand::new(archives.clone(), test_mapping());

        // Create archive with appspec that has no AfterInstall hooks
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("appspec.yml"), "version: 0.0\nos: linux\n").unwrap();

        let result = cmd.execute("AfterInstall", &test_spec()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn is_noop_returns_false_when_hooks_exist() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = HookCommand::new(archives.clone(), test_mapping());

        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\nhooks:\n  AfterInstall:\n    - location: scripts/run.sh\n      timeout: 60\n",
        )
        .unwrap();

        assert!(!cmd.is_noop("AfterInstall", &test_spec()).unwrap());
    }

    #[test]
    fn total_timeout_with_scripts() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = HookCommand::new(archives.clone(), test_mapping());

        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\nhooks:\n  AfterInstall:\n    - location: scripts/a.sh\n      timeout: 100\n    - location: scripts/b.sh\n      timeout: 200\n",
        )
        .unwrap();

        assert_eq!(cmd.total_timeout("AfterInstall", &test_spec()), Some(300));
    }

    #[test]
    fn create_executor_invalid_event_name() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let mut mapping = HashMap::new();
        mapping.insert("InvalidEvent".into(), vec!["InvalidEvent".into()]);
        let cmd = HookCommand::new(archives, mapping);

        let result = cmd.is_noop("InvalidEvent", &test_spec());
        assert!(result.is_err());
    }
}
