//! Hook command — runs lifecycle event scripts.
//!
//! Maps command names to lists of lifecycle event names, then creates and
//! executes a `LifecycleEventExecutor` for each event in sequence.

use crate::deployment_specification::types::DeploymentSpec;
use crate::host_command::DeploymentArchives;
use crate::lifecycle_event::{HookEnvPolicy, LifecycleEventExecutor, ScriptError};
use crate::logging::{LogConfig, SharedDeploymentLog};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, warn};

/// Maps command names to lists of lifecycle event names.
pub type HookMapping = HashMap<String, Vec<String>>;

#[derive(Debug)]
pub struct HookCommand {
    archives: Arc<DeploymentArchives>,
    hook_mapping: HookMapping,
    /// When set, every lifecycle/script log line is also written to
    /// `<root_dir>/deployment-logs/<program_name>-deployments.log` (the
    /// documented `CodeDeploy` deployment log). `None` disables it
    /// (`enable_deployments_log: false`, and the default in tests). One handle
    /// is shared by all deployments.
    deployment_log: Option<SharedDeploymentLog>,
    /// Opt-in hook-env hardening, from `strip_loader_env_in_hooks` /
    /// `restrict_hook_env_to_allowlist`. Default (all-`false`) = full inheritance.
    env_policy: HookEnvPolicy,
    /// Opt-in: reject hooks whose `location` escapes the deployment archive,
    /// from `reject_path_traversal_in_bundle`. Default `false`, preserving
    /// backwards-compatible behavior.
    reject_path_traversal: bool,
    /// Mode policy for per-deployment `logs/scripts.log`, from
    /// `restrict_agent_dir_permissions`. Default `false`, preserving
    /// backwards-compatible modes.
    restrict_log_permissions: bool,
}

impl HookCommand {
    #[must_use]
    pub fn new(archives: Arc<DeploymentArchives>, hook_mapping: HookMapping) -> Self {
        Self {
            archives,
            hook_mapping,
            deployment_log: None,
            env_policy: HookEnvPolicy::default(),
            reject_path_traversal: false,
            restrict_log_permissions: false,
        }
    }

    /// Set the mode policy for per-deployment log files, from
    /// `restrict_agent_dir_permissions`. Defaults to `false`, preserving
    /// backwards-compatible modes.
    #[must_use]
    pub fn with_restrict_log_permissions(mut self, restrict: bool) -> Self {
        self.restrict_log_permissions = restrict;
        self
    }

    /// Set the opt-in hook-environment hardening policy. Defaults to full env
    /// inheritance when not called.
    #[must_use]
    pub fn with_env_policy(mut self, env_policy: HookEnvPolicy) -> Self {
        self.env_policy = env_policy;
        self
    }

    /// Enable opt-in rejection of hooks whose `location` escapes the deployment
    /// archive. Defaults to `false` (backwards-compatible) when not called.
    #[must_use]
    pub fn with_reject_path_traversal(mut self, reject: bool) -> Self {
        self.reject_path_traversal = reject;
        self
    }

    /// Enable the per-deployment log (`deployment-logs/…-deployments.log`).
    ///
    /// Called by the dispatcher when `enable_deployments_log` is set. The
    /// `LogConfig`'s `root_dir`/`program_name` determine the log location.
    #[must_use]
    pub fn with_deployment_log_config(mut self, config: LogConfig) -> Self {
        self.deployment_log = Some(SharedDeploymentLog::new(config));
        self
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

        // Open the deployment log up front (best-effort) so the
        // `deployment-logs/` directory and `…-deployments.log` file exist for
        // the deployment even before the first line is written. A failure here
        // must not fail the deployment — it's a diagnostic side-channel.
        if let Some(log) = &self.deployment_log
            && let Err(e) = log.ensure_open()
        {
            warn!("Could not open deployment log: {e}");
        }

        // Lines reach the deployment log as each one is written to
        // `scripts.log`, so a failing event needs no extra writes here and
        // the deployment log gets no failure-summary line.
        let mut all_logs = Vec::new();
        for event_name in events {
            let executor = self.create_executor(event_name, spec)?;
            all_logs.extend(executor.execute()?);
        }
        Ok(all_logs)
    }

    /// Check if a command has any lifecycle events mapped.
    ///
    /// Pure lookup on `hook_mapping` — does not parse the `AppSpec`. Any
    /// `AppSpec` parse errors are surfaced later inside `execute_and_report`.
    #[must_use]
    pub fn is_noop(&self, command_name: &str, _spec: &DeploymentSpec) -> bool {
        let Some(events) = self.hook_mapping.get(command_name) else {
            info!("Command {command_name} is not in hook mapping; treating as noop.");
            return true;
        };

        if events.is_empty() {
            info!("Command {command_name} has no lifecycle events mapped; treating as noop.");
            return true;
        }

        info!(
            "Command {command_name} has {} lifecycle event(s) mapped; non-noop.",
            events.len()
        );
        false
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

        // Standard event names parse into their enum variant; any other name
        // (a custom event from `deploy-local --events`) becomes `Custom`.
        // `FromStr` stays strict, so the service polling path is unaffected.
        let lifecycle_event = event_name.parse().unwrap_or_else(|_: String| {
            crate::lifecycle_event::LifecycleEventType::Custom(event_name.to_string())
        });

        LifecycleEventExecutor::new(
            lifecycle_event,
            spec,
            &deploy_dir,
            last_successful.as_deref(),
            most_recent.as_deref(),
        )
        .map(|executor| {
            executor
                .with_env_policy(self.env_policy)
                .with_reject_path_traversal(self.reject_path_traversal)
                .with_restrict_log_permissions(self.restrict_log_permissions)
                .with_deployment_log(
                    self.deployment_log.as_ref().map(|log| log.for_deployment(&spec.deployment_id)),
                )
        })
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
            reuse_archive_from_deployment_id: None,
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
        assert!(cmd.is_noop("Unknown", &test_spec()));
    }

    #[test]
    fn is_noop_no_archive_dir() {
        let dir = TempDir::new().unwrap();
        let cmd = HookCommand::new(test_archives(&dir), test_mapping());
        // `is_noop` is a mapping-only check, so AppSpec/archive
        // presence is irrelevant — AfterInstall is mapped, so non-noop.
        assert!(!cmd.is_noop("AfterInstall", &test_spec()));
    }

    #[test]
    fn is_noop_empty_events_list() {
        // A command name with an empty event list in the mapping is a noop.
        let dir = TempDir::new().unwrap();
        let mut mapping = HashMap::new();
        mapping.insert("EmptyHook".to_string(), Vec::<String>::new());
        let cmd = HookCommand::new(test_archives(&dir), mapping);
        assert!(cmd.is_noop("EmptyHook", &test_spec()));
    }

    // Regression: `is_noop` must not touch the AppSpec. A malformed
    // AppSpec in the archive must not cause `is_noop` to error — the
    // error would bypass the completion pipeline and strand the
    // deployment as InProgress.
    #[test]
    fn is_noop_ignores_malformed_appspec() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = HookCommand::new(archives.clone(), test_mapping());

        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        // AppSpec with an invalid MLS range — would fail AppSpec::parse.
        fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\npermissions:\n  - object: /tmp\n    context:\n      type: object_t\n      range: invalid\n",
        )
        .unwrap();

        // is_noop must succeed (no AppSpec parse) and report non-noop
        // because the mapping has events for AfterInstall.
        assert!(
            !cmd.is_noop("AfterInstall", &test_spec()),
            "is_noop must return false when mapping has events, regardless of AppSpec state"
        );
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

        assert!(!cmd.is_noop("AfterInstall", &test_spec()));
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
    fn is_noop_with_invalid_event_name_reports_non_noop() {
        // Invalid lifecycle event names are not validated in `is_noop` —
        // the mapping has events, so the command is non-noop. The invalid
        // name surfaces later inside `execute_and_report`.
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let mut mapping = HashMap::new();
        mapping.insert("InvalidEvent".into(), vec!["InvalidEvent".into()]);
        let cmd = HookCommand::new(archives, mapping);

        let result = cmd.is_noop("InvalidEvent", &test_spec());
        assert!(!result, "Mapping with events implies non-noop");
    }

    #[cfg(unix)]
    fn log_config_for(root: &std::path::Path) -> LogConfig {
        LogConfig {
            log_dir: root.to_path_buf(),
            verbose: false,
            program_name: "codedeploy-agent".into(),
            root_dir: root.to_path_buf(),
            restrict_permissions: false,
            restrict_log_permissions: false,
        }
    }

    /// Write an `AfterInstall` hook running `script_body` into the archive of
    /// `deployment_id`, and return the `HookCommand` spec for it.
    #[cfg(unix)]
    fn stage_after_install(
        archives: &DeploymentArchives,
        deployment_id: &str,
        script_body: &str,
    ) -> DeploymentSpec {
        use std::os::unix::fs::PermissionsExt;

        let archive_dir = archives.archive_dir("dg-1", deployment_id);
        let scripts = archive_dir.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\nhooks:\n  AfterInstall:\n    - location: scripts/run.sh\n      timeout: 30\n",
        )
        .unwrap();
        fs::write(scripts.join("run.sh"), format!("#!/bin/sh\n{script_body}")).unwrap();
        fs::set_permissions(scripts.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        DeploymentSpec { deployment_id: deployment_id.into(), ..test_spec() }
    }

    /// Lines of the deployment log under `root`, with the
    /// `[YYYY-MM-DD HH:MM:SS.mmm] ` prefix checked and removed.
    #[cfg(unix)]
    fn deployment_log_messages(root: &std::path::Path) -> Vec<String> {
        let body =
            fs::read_to_string(root.join("deployment-logs/codedeploy-agent-deployments.log"))
                .unwrap();
        body.lines()
            .map(|line| {
                let (ts, msg) = line
                    .strip_prefix('[')
                    .and_then(|l| l.split_once("] "))
                    .unwrap_or_else(|| panic!("line lacks a [timestamp] prefix: {line:?}"));
                assert!(
                    chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S%.3f").is_ok(),
                    "bad timestamp {ts:?} in {line:?}"
                );
                msg.to_string()
            })
            .collect()
    }

    /// Each line is written as
    /// `[<ts>] [<deployment_id>]<message stripped>`, once, with no second
    /// timestamp and no blank line, and stdout/stderr are included.
    #[cfg(unix)]
    #[test]
    fn deployment_log_uses_one_record_per_script_line() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let root = dir.path().join("deployments");
        let cmd = HookCommand::new(archives.clone(), test_mapping())
            .with_deployment_log_config(log_config_for(&root));
        let spec = stage_after_install(&archives, "d-123", "echo hi\necho oops >&2\n");

        cmd.execute("AfterInstall", &spec).unwrap();

        let mut messages = deployment_log_messages(&root);
        // stdout and stderr are read by separate tasks, so their relative
        // order is not deterministic; sort the output lines.
        messages[2..].sort();
        assert_eq!(
            messages,
            vec![
                "[d-123]LifecycleEvent - AfterInstall",
                "[d-123]Script - scripts/run.sh",
                "[d-123][stderr]oops",
                "[d-123][stdout]hi",
            ]
        );
    }

    /// The deployment log must hold all script output, not the 2048-byte
    /// diagnostic tail that is sent to the service.
    #[cfg(unix)]
    #[test]
    fn deployment_log_keeps_output_beyond_diagnostic_tail() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let root = dir.path().join("deployments");
        let cmd = HookCommand::new(archives.clone(), test_mapping())
            .with_deployment_log_config(log_config_for(&root));
        // 200 lines of ~40 bytes is well past the 2048-byte tail.
        let spec = stage_after_install(
            &archives,
            "d-123",
            "i=0; while [ $i -lt 200 ]; do echo \"line $i padded to be longer\"; i=$((i+1)); done\n",
        );

        cmd.execute("AfterInstall", &spec).unwrap();

        let messages = deployment_log_messages(&root);
        assert_eq!(messages.len(), 202, "header + script line + 200 output lines");
        assert_eq!(messages[0], "[d-123]LifecycleEvent - AfterInstall");
        assert_eq!(messages[2], "[d-123][stdout]line 0 padded to be longer");
        assert_eq!(messages[201], "[d-123][stdout]line 199 padded to be longer");
    }

    /// When the deployment log is NOT configured (the default, i.e.
    /// `enable_deployments_log: false`), no `deployment-logs/` dir is created.
    #[cfg(unix)]
    #[test]
    fn deployment_log_absent_when_disabled() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = HookCommand::new(archives.clone(), test_mapping()); // no log config

        let archive_dir = archives.archive_dir("dg-1", "d-123");
        let scripts = archive_dir.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\nhooks:\n  AfterInstall:\n    - location: scripts/ok.sh\n      timeout: 10\n",
        )
        .unwrap();
        fs::write(scripts.join("ok.sh"), "#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(scripts.join("ok.sh"), fs::Permissions::from_mode(0o755)).unwrap();

        cmd.execute("AfterInstall", &test_spec()).unwrap();

        assert!(
            !dir.path().join("deployments/deployment-logs").exists(),
            "no deployment-logs dir should be created when disabled"
        );
    }

    /// A failing script's output must be in the deployment log, and no
    /// failure-summary line is added: the deployment log holds only script
    /// log lines.
    #[cfg(unix)]
    #[test]
    fn deployment_log_keeps_failing_script_output_without_summary_line() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let root = dir.path().join("deployments");
        let cmd = HookCommand::new(archives.clone(), test_mapping())
            .with_deployment_log_config(log_config_for(&root));
        let spec = stage_after_install(&archives, "d-123", "echo before-failure\nexit 1\n");

        let result = cmd.execute("AfterInstall", &spec);
        assert!(result.is_err(), "failing script must propagate an error");

        assert_eq!(
            deployment_log_messages(&root),
            vec![
                "[d-123]LifecycleEvent - AfterInstall",
                "[d-123]Script - scripts/run.sh",
                "[d-123][stdout]before-failure",
            ]
        );
    }

    /// Concurrent deployments share one deployment log handle: every record stays whole and is tagged with
    /// its own deployment ID.
    #[cfg(unix)]
    #[test]
    fn concurrent_deployments_share_one_deployment_log() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let root = dir.path().join("deployments");
        let cmd = HookCommand::new(archives.clone(), test_mapping())
            .with_deployment_log_config(log_config_for(&root));
        let script = "i=0; while [ $i -lt 50 ]; do echo \"out $i\"; i=$((i+1)); done\n";
        let specs: Vec<_> = ["d-aaa", "d-bbb"]
            .iter()
            .map(|id| stage_after_install(&archives, id, script))
            .collect();

        std::thread::scope(|scope| {
            for spec in &specs {
                let cmd = &cmd;
                scope.spawn(move || cmd.execute("AfterInstall", spec).unwrap());
            }
        });

        let messages = deployment_log_messages(&root);
        for id in ["d-aaa", "d-bbb"] {
            let mine: Vec<_> =
                messages.iter().filter(|m| m.starts_with(&format!("[{id}]"))).collect();
            assert_eq!(mine.len(), 52, "{id}: header + script line + 50 output lines");
            assert_eq!(*mine[0], format!("[{id}]LifecycleEvent - AfterInstall"));
            assert_eq!(*mine[51], format!("[{id}][stdout]out 49"));
        }
        assert_eq!(messages.len(), 104, "no stray or split records: {messages:?}");
    }

    /// If the deployment log can't be opened (its dir can't be created), the
    /// deployment still succeeds — the log is a best-effort side-channel.
    /// Exercises the `Err(e) => warn!(...)` arm of the logger open.
    #[cfg(unix)]
    #[test]
    fn deployment_log_open_failure_does_not_break_deployment() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);

        // Point the deployment-log root at a path *under a regular file*, so
        // `create_dir_secure(<file>/deployment-logs)` fails with ENOTDIR.
        let blocker = dir.path().join("not-a-dir");
        fs::write(&blocker, "x").unwrap();
        let bad_root = blocker.join("sub");
        let cmd = HookCommand::new(archives.clone(), test_mapping())
            .with_deployment_log_config(log_config_for(&bad_root));

        // A normal, succeeding hook.
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        let scripts = archive_dir.join("scripts");
        fs::create_dir_all(&scripts).unwrap();
        fs::write(
            archive_dir.join("appspec.yml"),
            "version: 0.0\nos: linux\nhooks:\n  AfterInstall:\n    - location: scripts/ok.sh\n      timeout: 10\n",
        )
        .unwrap();
        fs::write(scripts.join("ok.sh"), "#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(scripts.join("ok.sh"), fs::Permissions::from_mode(0o755)).unwrap();

        // Deployment must still succeed despite the unopenable log.
        let result = cmd.execute("AfterInstall", &test_spec());
        assert!(result.is_ok(), "deployment must not fail when the log can't open: {result:?}");
        assert!(!bad_root.join("deployment-logs").exists());
    }

    #[test]
    fn execute_with_custom_event_name_runs_as_noop_when_no_hook() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);

        // Create archive dir with an appspec that has no matching hook.
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("appspec.yml"), "version: 0.0\nos: linux\n").unwrap();

        // A non-standard ("custom") event name is accepted and runs as a custom
        // event rather than erroring; with no matching appspec hook it is a
        // noop. `codedeploy-local` relies on custom events being runnable.
        let mut mapping = HashMap::new();
        mapping.insert("HealthCheck".into(), vec!["HealthCheck".into()]);
        let cmd = HookCommand::new(archives, mapping);

        let result = cmd.execute("HealthCheck", &test_spec());
        assert!(result.is_ok(), "custom event should not error: {result:?}");
        assert!(result.unwrap().is_empty(), "custom event with no hook is a noop");
    }
}
