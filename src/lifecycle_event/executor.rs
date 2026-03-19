//! @risk medium
//!
//! Hook executor — orchestrates running lifecycle event scripts.
//!
//! Selects the correct deployment directory, parses the appspec, and executes
//! each script for a given lifecycle event in order.
//!
//! ## Not yet implemented
//!
//! - **Deployment log**: Writes to a secondary `DeploymentLog` when
//!   `enable_deployments_log` config is set. `DeploymentLogger` exists in `logging/`
//!   but is not wired into the executor yet. Needs config system integration.

use super::LifecycleEventType;
use super::deployment_selector::select_deployment_dir;
use super::error::{ErrorCode, ScriptError};
use super::script::Script;
use super::script_run_log::ScriptRunLog;
use crate::application_specification::AppSpec;
use crate::deployment_specification::types::{DeploymentSpec, RevisionLocation, RevisionSource};
use crate::system::file_ops::ensure_executable;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, info};

#[derive(Debug)]
pub struct LifecycleEventExecutor {
    lifecycle_event: LifecycleEventType,
    current_deployment_root_dir: PathBuf,
    deployment_archive_dir: Option<PathBuf>,
    app_spec: Option<AppSpec>,
    child_envs: HashMap<String, String>,
}

impl LifecycleEventExecutor {
    /// Create a new `HookExecutor` for a single lifecycle event.
    ///
    /// Selects the correct deployment directory, parses the appspec, and builds
    /// environment variables for child scripts.
    ///
    /// # Errors
    /// Returns an error if the appspec file exists but cannot be parsed.
    pub fn new(
        lifecycle_event: LifecycleEventType,
        spec: &DeploymentSpec,
        deployment_root_dir: &Path,
        last_successful_dir: Option<&Path>,
        most_recent_dir: Option<&Path>,
    ) -> Result<Self, ScriptError> {
        let deployment_type =
            spec.deployment_type.parse().unwrap_or(super::DeploymentType::InPlace);

        let selected_dir = select_deployment_dir(
            lifecycle_event,
            &spec.deployment_creator,
            deployment_type,
            deployment_root_dir,
            last_successful_dir,
            most_recent_dir,
        );

        let archive_dir = selected_dir.join("deployment-archive");

        let mut app_spec = None;
        if archive_dir.exists() {
            app_spec = Some(parse_app_spec(&archive_dir, &spec.app_spec_path)?);
        }

        let child_envs = build_child_envs(lifecycle_event, spec);

        Ok(Self {
            lifecycle_event,
            current_deployment_root_dir: deployment_root_dir.to_path_buf(),
            deployment_archive_dir: if archive_dir.exists() {
                Some(archive_dir)
            } else {
                None
            },
            app_spec,
            child_envs,
        })
    }

    /// Returns true if there are no scripts to run for this lifecycle event.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        match &self.app_spec {
            None => true,
            Some(spec) => {
                let event_name = self.lifecycle_event.to_string();
                spec.hooks().get(&event_name).is_empty()
            },
        }
    }

    /// Sum of all script timeouts for this lifecycle event.
    /// Returns `None` if noop (no scripts).
    #[must_use]
    pub fn total_timeout(&self) -> Option<u64> {
        if self.is_noop() {
            return None;
        }
        let event_name = self.lifecycle_event.to_string();
        let scripts = self.app_spec.as_ref()?.hooks().get(&event_name);
        Some(scripts.iter().map(|s| u64::from(s.timeout())).sum())
    }

    /// Execute all scripts for this lifecycle event in order.
    ///
    /// Returns the bounded log entries for diagnostics.
    ///
    /// # Panics
    /// Panics if the internal mutex is poisoned (only possible if a prior panic occurred).
    ///
    /// # Errors
    /// Returns a `ScriptError` if any script fails.
    pub fn execute(&self) -> Result<Vec<String>, ScriptError> {
        let event_name = self.lifecycle_event.to_string();

        let scripts = match &self.app_spec {
            Some(spec) => spec.hooks().get(&event_name),
            None => return Ok(Vec::new()),
        };

        if scripts.is_empty() {
            return Ok(Vec::new());
        }

        info!(
            event = %event_name,
            script_count = scripts.len(),
            "Executing lifecycle event"
        );

        let log_path = self.current_deployment_root_dir.join("logs/scripts.log");
        let log = Arc::new(Mutex::new(
            ScriptRunLog::open(&log_path).unwrap_or_else(|_| ScriptRunLog::in_memory()),
        ));

        log.lock().unwrap().write_line("", &format!("LifecycleEvent - {event_name}"));

        let archive_dir = self.deployment_archive_dir.as_ref().unwrap();

        for script_info in scripts {
            self.execute_script(script_info, archive_dir, &log)?;
        }

        Ok(log.lock().unwrap().entries())
    }

    fn execute_script(
        &self,
        script_info: &crate::application_specification::ScriptInfo,
        archive_dir: &Path,
        log: &Arc<Mutex<ScriptRunLog>>,
    ) -> Result<(), ScriptError> {
        let location = script_info.location().to_string();
        let script_path = archive_dir.join(&location);
        let err = |code, msg: String| ScriptError::new(code, location.clone(), Vec::new(), msg);

        log.lock().unwrap().write_line("", &format!("Script - {location}"));

        debug!(script = %location, "Running lifecycle script");

        if !script_path.exists() {
            return Err(err(
                ErrorCode::ScriptMissing,
                format!("Script does not exist at specified location: {}", script_path.display()),
            ));
        }

        if let Err(e) = ensure_executable(&script_path) {
            return Err(err(
                ErrorCode::ScriptExecutability,
                format!(
                    "Unable to set script at specified location: {location} as executable: {e}"
                ),
            ));
        }

        let timeout = Duration::from_secs(u64::from(script_info.timeout()));
        let script = Script::new(
            script_path,
            script_info.runas().map(String::from),
            false,
            &self.child_envs,
            Arc::clone(log),
        );

        let exit_code = match script.execute(timeout) {
            Ok(code) => code,
            Err(e) if e == "timeout" => {
                return Err(err(
                    ErrorCode::ScriptTimedOut,
                    format!(
                        "Script at specified location: {location} failed to complete in {} seconds",
                        script_info.timeout()
                    ),
                ));
            },
            Err(e) if e == "outputs_left_open" => {
                return Err(err(
                    ErrorCode::OutputsLeftOpen,
                    format!("Script at specified location: {location} failed to close STDOUT"),
                ));
            },
            Err(e) => {
                return Err(err(
                    ErrorCode::ScriptFailed,
                    format!("Script at specified location: {location} failed with error {e}"),
                ));
            },
        };

        if exit_code != 0 {
            let who = match script_info.runas() {
                Some(user) => format!("{location} run as user {user}"),
                None => location.clone(),
            };
            return Err(err(
                ErrorCode::ScriptFailed,
                format!("Script at specified location: {who} failed with exit code {exit_code}"),
            ));
        }

        Ok(())
    }
}

fn parse_app_spec(archive_dir: &Path, app_spec_path: &str) -> Result<AppSpec, ScriptError> {
    let path = archive_dir.join(app_spec_path);

    let mut error_msg = format!(
        "The CodeDeploy agent did not find an AppSpec file within the unpacked revision \
         directory at revision-relative path \"{app_spec_path}\". The revision was unpacked \
         to directory \"{}\", and the AppSpec file was expected but not found at path \
         \"{}\". Consult the AWS CodeDeploy Appspec documentation for more information at \
         http://docs.aws.amazon.com/codedeploy/latest/userguide/reference-appspec-file.html",
        archive_dir.display(),
        path.display()
    );

    let Ok(contents) = std::fs::read_to_string(&path) else {
        return Err(ScriptError::new(
            ErrorCode::UnknownError,
            String::new(),
            Vec::new(),
            error_msg,
        ));
    };

    match AppSpec::parse(&contents) {
        Ok(spec) => Ok(spec),
        Err(e) => {
            error_msg = e.to_string();
            Err(ScriptError::new(ErrorCode::UnknownError, String::new(), Vec::new(), error_msg))
        },
    }
}

fn build_child_envs(
    lifecycle_event: LifecycleEventType,
    spec: &DeploymentSpec,
) -> HashMap<String, String> {
    let mut envs = HashMap::new();
    envs.insert("LIFECYCLE_EVENT".into(), lifecycle_event.to_string());
    envs.insert("DEPLOYMENT_ID".into(), spec.deployment_id.clone());
    envs.insert("APPLICATION_NAME".into(), spec.application_name.clone());
    envs.insert("DEPLOYMENT_GROUP_NAME".into(), spec.deployment_group_name.clone());
    envs.insert("DEPLOYMENT_GROUP_ID".into(), spec.deployment_group_id.clone());

    match (&spec.revision_source, &spec.revision) {
        (RevisionSource::S3, RevisionLocation::S3 { bucket, key, version, etag, .. }) => {
            envs.insert("BUNDLE_BUCKET".into(), bucket.clone());
            envs.insert("BUNDLE_KEY".into(), key.clone());
            if let Some(v) = version {
                envs.insert("BUNDLE_VERSION".into(), v.clone());
            }
            if let Some(e) = etag {
                envs.insert("BUNDLE_ETAG".into(), e.clone());
            }
        },
        (RevisionSource::GitHub, RevisionLocation::GitHub { commit_id, .. }) => {
            envs.insert("BUNDLE_COMMIT".into(), commit_id.clone());
        },
        _ => {},
    }

    envs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{
        DeploymentSpec, RevisionLocation, RevisionSource,
    };
    use tempfile::TempDir;

    fn make_spec(revision_source: RevisionSource, revision: RevisionLocation) -> DeploymentSpec {
        DeploymentSpec {
            deployment_id: "d-123".into(),
            deployment_group_id: "dg-456".into(),
            deployment_group_name: "MyGroup".into(),
            application_name: "MyApp".into(),
            deployment_creator: "user".into(),
            deployment_type: "IN_PLACE".into(),
            app_spec_path: "appspec.yml".into(),
            file_exists_behavior: "DISALLOW".into(),
            revision_source,
            revision,
            all_possible_lifecycle_events: None,
        }
    }

    fn s3_spec() -> DeploymentSpec {
        make_spec(
            RevisionSource::S3,
            RevisionLocation::S3 {
                bucket: "my-bucket".into(),
                key: "my-key".into(),
                bundle_type: "zip".into(),
                version: Some("v1".into()),
                etag: Some("abc".into()),
            },
        )
    }

    fn github_spec() -> DeploymentSpec {
        make_spec(
            RevisionSource::GitHub,
            RevisionLocation::GitHub {
                account: "acme".into(),
                repository: "app".into(),
                commit_id: "sha123".into(),
                anonymous: true,
                auth_token: None,
                bundle_type: None,
            },
        )
    }

    fn local_spec() -> DeploymentSpec {
        make_spec(
            RevisionSource::LocalFile,
            RevisionLocation::Local {
                location: "/tmp/bundle.zip".into(),
                bundle_type: "zip".into(),
            },
        )
    }

    fn setup_appspec(dir: &Path, content: &str) {
        let archive = dir.join("deployment-archive");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(archive.join("appspec.yml"), content).unwrap();
    }

    const APPSPEC_WITH_HOOKS: &str = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
      timeout: 300
    - location: scripts/verify.sh
      timeout: 60
";

    // --- build_child_envs ---

    #[test]
    fn build_envs_s3() {
        let spec = s3_spec();
        let envs = build_child_envs(LifecycleEventType::AfterInstall, &spec);
        assert_eq!(envs["LIFECYCLE_EVENT"], "AfterInstall");
        assert_eq!(envs["DEPLOYMENT_ID"], "d-123");
        assert_eq!(envs["APPLICATION_NAME"], "MyApp");
        assert_eq!(envs["DEPLOYMENT_GROUP_NAME"], "MyGroup");
        assert_eq!(envs["DEPLOYMENT_GROUP_ID"], "dg-456");
        assert_eq!(envs["BUNDLE_BUCKET"], "my-bucket");
        assert_eq!(envs["BUNDLE_KEY"], "my-key");
        assert_eq!(envs["BUNDLE_VERSION"], "v1");
        assert_eq!(envs["BUNDLE_ETAG"], "abc");
    }

    #[test]
    fn build_envs_github() {
        let spec = github_spec();
        let envs = build_child_envs(LifecycleEventType::BeforeInstall, &spec);
        assert_eq!(envs["BUNDLE_COMMIT"], "sha123");
        assert!(!envs.contains_key("BUNDLE_BUCKET"));
    }

    #[test]
    fn build_envs_local() {
        let spec = local_spec();
        let envs = build_child_envs(LifecycleEventType::BeforeInstall, &spec);
        assert!(!envs.contains_key("BUNDLE_BUCKET"));
        assert!(!envs.contains_key("BUNDLE_COMMIT"));
    }

    // --- parse_app_spec ---

    #[test]
    fn parse_app_spec_missing_file() {
        let dir = TempDir::new().unwrap();
        let result = parse_app_spec(dir.path(), "appspec.yml");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error_code, ErrorCode::UnknownError);
        assert!(err.message.contains("did not find an AppSpec file"));
    }

    #[test]
    fn parse_app_spec_invalid_yaml() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("appspec.yml"), "not: valid: yaml: {{").unwrap();
        let result = parse_app_spec(dir.path(), "appspec.yml");
        assert!(result.is_err());
    }

    #[test]
    fn parse_app_spec_valid() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("appspec.yml"), APPSPEC_WITH_HOOKS).unwrap();
        let result = parse_app_spec(dir.path(), "appspec.yml");
        assert!(result.is_ok());
    }

    // --- new ---

    #[test]
    fn new_no_archive_dir() {
        let dir = TempDir::new().unwrap();
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        assert!(he.is_noop());
        assert!(he.app_spec.is_none());
    }

    #[test]
    fn new_with_appspec() {
        let dir = TempDir::new().unwrap();
        setup_appspec(dir.path(), APPSPEC_WITH_HOOKS);
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        assert!(!he.is_noop());
    }

    // --- is_noop ---

    #[test]
    fn is_noop_no_appspec() {
        let dir = TempDir::new().unwrap();
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        assert!(he.is_noop());
    }

    #[test]
    fn is_noop_no_hooks_for_event() {
        let dir = TempDir::new().unwrap();
        setup_appspec(dir.path(), APPSPEC_WITH_HOOKS);
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::ApplicationStop,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        assert!(he.is_noop());
    }

    #[test]
    fn is_noop_has_hooks() {
        let dir = TempDir::new().unwrap();
        setup_appspec(dir.path(), APPSPEC_WITH_HOOKS);
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        assert!(!he.is_noop());
    }

    // --- total_timeout ---

    #[test]
    fn total_timeout_noop() {
        let dir = TempDir::new().unwrap();
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        assert_eq!(he.total_timeout(), None);
    }

    #[test]
    fn total_timeout_sums_scripts() {
        let dir = TempDir::new().unwrap();
        setup_appspec(dir.path(), APPSPEC_WITH_HOOKS);
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        assert_eq!(he.total_timeout(), Some(360)); // 300 + 60
    }

    // --- execute ---

    #[test]
    fn execute_noop_returns_empty() {
        let dir = TempDir::new().unwrap();
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        let result = he.execute().unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn execute_missing_script_returns_error() {
        let dir = TempDir::new().unwrap();
        setup_appspec(dir.path(), APPSPEC_WITH_HOOKS);
        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        let result = he.execute();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error_code, ErrorCode::ScriptMissing);
    }

    #[cfg(unix)]
    #[test]
    fn execute_successful_script() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let appspec = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/ok.sh
      timeout: 10
";
        setup_appspec(dir.path(), appspec);

        let scripts_dir = dir.path().join("deployment-archive/scripts");
        std::fs::create_dir_all(&scripts_dir).unwrap();
        std::fs::write(scripts_dir.join("ok.sh"), "#!/bin/sh\necho done\n").unwrap();
        std::fs::set_permissions(scripts_dir.join("ok.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        let entries = he.execute().unwrap();
        assert!(entries.iter().any(|e| e.contains("LifecycleEvent - AfterInstall")));
        assert!(entries.iter().any(|e| e.contains("Script - scripts/ok.sh")));
    }

    #[cfg(unix)]
    #[test]
    fn execute_failing_script() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let appspec = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/fail.sh
      timeout: 10
";
        setup_appspec(dir.path(), appspec);

        let scripts_dir = dir.path().join("deployment-archive/scripts");
        std::fs::create_dir_all(&scripts_dir).unwrap();
        std::fs::write(scripts_dir.join("fail.sh"), "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(
            scripts_dir.join("fail.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();

        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        let result = he.execute();
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.error_code, ErrorCode::ScriptFailed);
        assert!(err.message.contains("exit code 1"));
    }
}
