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
use super::script::{HookEnvPolicy, Script};
use super::script_run_log::ScriptRunLog;
use crate::application_specification::AppSpec;
use crate::deployment_specification::types::{DeploymentSpec, RevisionLocation, RevisionSource};
use crate::paths::APPSPEC_PATH_SEPARATORS;
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
    /// Opt-in hook-env hardening. Default (all-`false`) = full inheritance.
    /// Set via [`Self::with_env_policy`].
    env_policy: HookEnvPolicy,
    /// Opt-in: reject a hook whose `location` resolves outside the deployment
    /// archive. Default `false`, preserving backwards-compatible behavior.
    /// Reuses the same `reject_path_traversal_in_bundle` flag as the
    /// installer's `source` check.
    reject_path_traversal: bool,
    /// Mode policy for `logs/scripts.log` and its parent dir, from
    /// `restrict_agent_dir_permissions`. Default `false` (world-readable
    /// 0755/0644); `true` = hardened 0750/0640.
    restrict_log_permissions: bool,
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
            &lifecycle_event,
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

        let child_envs = build_child_envs(&lifecycle_event, spec, deployment_root_dir);

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
            env_policy: HookEnvPolicy::default(),
            reject_path_traversal: false,
            restrict_log_permissions: false,
        })
    }

    /// Set the opt-in hook-environment hardening policy. Defaults to
    /// [`HookEnvPolicy::default`] (full env inheritance).
    #[must_use]
    pub fn with_env_policy(mut self, env_policy: HookEnvPolicy) -> Self {
        self.env_policy = env_policy;
        self
    }

    /// Enable opt-in rejection of hooks whose `location` escapes the deployment
    /// archive. Defaults to `false`, preserving backwards-compatible behavior.
    #[must_use]
    pub fn with_reject_path_traversal(mut self, reject: bool) -> Self {
        self.reject_path_traversal = reject;
        self
    }

    /// Set the mode policy for `logs/scripts.log`, from
    /// `restrict_agent_dir_permissions`. Defaults to `false` (0755/0644).
    #[must_use]
    pub fn with_restrict_log_permissions(mut self, restrict: bool) -> Self {
        self.restrict_log_permissions = restrict;
        self
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
            ScriptRunLog::open_with_policy(&log_path, self.restrict_log_permissions)
                .unwrap_or_else(|_| ScriptRunLog::in_memory()),
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
        // Strip leading separators so `join` cannot discard the archive dir.
        // Mirrors the `files.source` strip in installer/core.rs.
        let location_relative = location.trim_start_matches(APPSPEC_PATH_SEPARATORS);
        let script_path = archive_dir.join(location_relative);
        // `entries()` is the bounded stdout/stderr tail the stream tasks buffered;
        // it becomes the diagnostic's log so the service sees the script's output.
        let err = |code, msg: String| {
            let log_tail = log.lock().map(|l| l.entries()).unwrap_or_default();
            ScriptError::new(code, location.clone(), log_tail, msg)
        };

        log.lock().unwrap().write_line("", &format!("Script - {location}"));

        debug!(script = %location, "Running lifecycle script");

        // Reject a `location` that escapes the archive via `..`. Normalize
        // both sides LEXICALLY (resolve `.`/`..` without touching the
        // filesystem) rather than via `canonicalize`: `starts_with` treats
        // `..` as an ordinary component, so comparing un-normalized paths
        // (e.g. `<archive>/../../etc/evil`) would spuriously pass containment,
        // and `canonicalize` fails on a nonexistent target — which would turn
        // a merely-missing script into a misleading "resolves outside" error
        // instead of the precise "does not exist" reported below. Lexical
        // normalization is existence-independent and still rejects the escape.
        // Mirrors the `files.source` check in installer/core.rs (nu_path).
        if self.reject_path_traversal {
            let normalized_script = nu_path::expand_path(&script_path, true);
            let normalized_archive = nu_path::expand_path(archive_dir, true);
            if !normalized_script.starts_with(&normalized_archive) {
                return Err(err(
                    ErrorCode::ScriptMissing,
                    format!(
                        "Script at specified location: {location} resolves outside the deployment archive"
                    ),
                ));
            }
        }

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
        let script = self.build_script(script_info, script_path, log);

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

    /// Build a [`Script`] from an appspec
    /// [`ScriptInfo`](crate::application_specification::ScriptInfo).
    ///
    /// This is the single source of truth for translating appspec fields into
    /// [`Script`] constructor arguments.
    fn build_script(
        &self,
        script_info: &crate::application_specification::ScriptInfo,
        script_path: PathBuf,
        log: &Arc<Mutex<ScriptRunLog>>,
    ) -> Script {
        Script::with_env_policy(
            script_path,
            script_info.runas().map(String::from),
            script_info.sudo().unwrap_or(false),
            &self.child_envs,
            self.env_policy,
            Arc::clone(log),
        )
    }
}

/// Resolve the `AppSpec` in a revision's archive, tolerating a filename mismatch
/// across revisions.
///
/// Pre-DownloadBundle events run the previous revision's scripts but only know
/// the current deploy's `--appspec-filename`, so prefer the requested name then
/// fall back to `appspec.yaml`/`appspec.yml` (mirrors `install::resolve_appspec_path`).
fn resolve_appspec_path(archive_dir: &Path, app_spec_path: &str) -> PathBuf {
    let requested = archive_dir.join(app_spec_path);
    if requested.exists() {
        return requested;
    }
    let long_ext = archive_dir.join("appspec.yaml");
    if long_ext.exists() {
        return long_ext;
    }
    let short_ext = archive_dir.join("appspec.yml");
    if short_ext.exists() {
        return short_ext;
    }
    // Nothing found — return the requested path so the error names it.
    requested
}

fn parse_app_spec(archive_dir: &Path, app_spec_path: &str) -> Result<AppSpec, ScriptError> {
    let path = resolve_appspec_path(archive_dir, app_spec_path);

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
    lifecycle_event: &LifecycleEventType,
    spec: &DeploymentSpec,
    deployment_root_dir: &Path,
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
            // The service frequently sends a null ETag in the spec, so fall back
            // to the ETag the agent persisted on download.
            let resolved_etag = etag.clone().or_else(|| {
                let etag_path = deployment_root_dir.join(crate::host_command::BUNDLE_ETAG_FILE);
                std::fs::read_to_string(&etag_path)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            });
            if let Some(e) = resolved_etag {
                envs.insert("BUNDLE_ETAG".into(), e);
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
        let envs =
            build_child_envs(&LifecycleEventType::AfterInstall, &spec, Path::new("/nonexistent"));
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
    fn build_envs_s3_etag_fallback_from_persisted_file() {
        // Spec carries no etag (service sent null); the persisted .bundle-etag
        // file should supply BUNDLE_ETAG.
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(crate::host_command::BUNDLE_ETAG_FILE), "deadbeef123\n")
            .unwrap();
        let spec = make_spec(
            RevisionSource::S3,
            RevisionLocation::S3 {
                bucket: "b".into(),
                key: "k".into(),
                bundle_type: "zip".into(),
                version: None,
                etag: None,
            },
        );
        let envs = build_child_envs(&LifecycleEventType::AfterInstall, &spec, dir.path());
        assert_eq!(envs["BUNDLE_ETAG"], "deadbeef123");
    }

    #[test]
    fn build_envs_s3_spec_etag_wins_over_file() {
        // When the spec carries an etag it takes precedence over the file.
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join(crate::host_command::BUNDLE_ETAG_FILE), "fromfile").unwrap();
        let spec = s3_spec(); // etag: Some("abc")
        let envs = build_child_envs(&LifecycleEventType::AfterInstall, &spec, dir.path());
        assert_eq!(envs["BUNDLE_ETAG"], "abc");
    }

    #[test]
    fn build_envs_github() {
        let spec = github_spec();
        let envs =
            build_child_envs(&LifecycleEventType::BeforeInstall, &spec, Path::new("/nonexistent"));
        assert_eq!(envs["BUNDLE_COMMIT"], "sha123");
        assert!(!envs.contains_key("BUNDLE_BUCKET"));
    }

    #[test]
    fn build_envs_local() {
        let spec = local_spec();
        let envs =
            build_child_envs(&LifecycleEventType::BeforeInstall, &spec, Path::new("/nonexistent"));
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

    #[test]
    fn parse_app_spec_falls_back_when_requested_name_absent() {
        // Prior revision has only appspec.yml while the current deploy requested
        // my-spec.yml; resolution falls back to appspec.yml.
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("appspec.yml"), APPSPEC_WITH_HOOKS).unwrap();
        let result = parse_app_spec(dir.path(), "my-spec.yml");
        assert!(result.is_ok(), "should fall back to appspec.yml in the prior revision");
    }

    #[test]
    fn resolve_appspec_path_prefers_requested_name() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("my-spec.yml"), "version: 0.0\nos: linux\n").unwrap();
        std::fs::write(dir.path().join("appspec.yml"), "version: 0.0\nos: linux\n").unwrap();
        // Both present: the explicitly-requested name wins.
        assert_eq!(resolve_appspec_path(dir.path(), "my-spec.yml"), dir.path().join("my-spec.yml"));
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
    fn execute_leading_slash_location_resolves_inside_archive() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let appspec = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: /scripts/ok.sh
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
        assert!(entries.iter().any(|e| e.contains("Script - /scripts/ok.sh")));
    }

    /// Repeated leading separators must be fully stripped.
    #[cfg(unix)]
    #[test]
    fn execute_repeated_leading_slash_location_resolves_inside_archive() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let appspec = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: //scripts/ok.sh
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
        assert!(entries.iter().any(|e| e.contains("Script - //scripts/ok.sh")));
    }

    /// Uses a missing script and asserts the resolved path in the error message,
    /// so nothing has to be executed to check resolution.
    #[cfg(windows)]
    #[test]
    fn execute_leading_backslash_location_resolves_inside_archive() {
        let dir = TempDir::new().unwrap();
        let appspec = r"
version: 0.0
os: windows
hooks:
  AfterInstall:
    - location: \scripts\missing.cmd
      timeout: 10
";
        setup_appspec(dir.path(), appspec);

        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        let err = he.execute().unwrap_err();

        assert_eq!(err.error_code, ErrorCode::ScriptMissing);
        let expected = dir.path().join(r"deployment-archive\scripts\missing.cmd");
        assert!(
            err.message.contains(&expected.display().to_string()),
            "location must resolve inside the archive, got: {}",
            err.message
        );
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

    #[test]
    fn script_info_sudo_true_parses_through_to_accessor() {
        // Arrange
        let yaml = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
      sudo: true
";

        // Act
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("AfterInstall");

        // Assert
        assert_eq!(scripts[0].sudo(), Some(true));
    }

    #[test]
    fn script_info_sudo_false_parses_through_to_accessor() {
        // Arrange
        let yaml = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
      sudo: false
";

        // Act
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("AfterInstall");

        // Assert
        assert_eq!(scripts[0].sudo(), Some(false));
    }

    #[test]
    fn script_info_sudo_missing_is_none() {
        // Arrange
        let yaml = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
";

        // Act
        let spec = AppSpec::parse(yaml).unwrap();
        let scripts = spec.hooks().get("AfterInstall");

        // Assert
        assert_eq!(scripts[0].sudo(), None);
    }

    fn build_test_executor(appspec: &str) -> (TempDir, LifecycleEventExecutor) {
        let dir = TempDir::new().unwrap();
        setup_appspec(dir.path(), appspec);
        let spec = s3_spec();
        let executor = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap();
        (dir, executor)
    }

    fn sample_log() -> Arc<Mutex<ScriptRunLog>> {
        Arc::new(Mutex::new(ScriptRunLog::in_memory()))
    }

    #[test]
    fn build_script_forwards_sudo_true_from_appspec() {
        // Arrange
        let yaml = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
      sudo: true
";
        let (_dir, executor) = build_test_executor(yaml);
        let script_info = &executor.app_spec.as_ref().unwrap().hooks().get("AfterInstall")[0];

        // Act
        let script = executor.build_script(
            script_info,
            PathBuf::from("/archive/scripts/install.sh"),
            &sample_log(),
        );

        // Assert — the executor forwarded `sudo: true` into Script::new.
        assert!(script.sudo(), "executor must forward appspec sudo: true into Script");
        assert_eq!(script.runas(), None);
    }

    #[test]
    fn build_script_forwards_sudo_false_from_appspec() {
        // Arrange
        let yaml = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
      sudo: false
";
        let (_dir, executor) = build_test_executor(yaml);
        let script_info = &executor.app_spec.as_ref().unwrap().hooks().get("AfterInstall")[0];

        // Act
        let script = executor.build_script(
            script_info,
            PathBuf::from("/archive/scripts/install.sh"),
            &sample_log(),
        );

        // Assert
        assert!(!script.sudo());
    }

    #[test]
    fn build_script_defaults_missing_sudo_to_false() {
        // Arrange — no `sudo` key in the appspec; omitted sudo defaults to false.
        let yaml = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
";
        let (_dir, executor) = build_test_executor(yaml);
        let script_info = &executor.app_spec.as_ref().unwrap().hooks().get("AfterInstall")[0];

        // Act
        let script = executor.build_script(
            script_info,
            PathBuf::from("/archive/scripts/install.sh"),
            &sample_log(),
        );

        // Assert — omitted `sudo` must default to false.
        assert!(!script.sudo());
    }

    #[test]
    fn build_script_forwards_runas_and_sudo_together() {
        // Arrange — exercises the (runas=Some, sudo=true) cell of the
        // four-way switch in script::build_command.
        let yaml = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
      runas: deploy
      sudo: true
";
        let (_dir, executor) = build_test_executor(yaml);
        let script_info = &executor.app_spec.as_ref().unwrap().hooks().get("AfterInstall")[0];

        // Act
        let script = executor.build_script(
            script_info,
            PathBuf::from("/archive/scripts/install.sh"),
            &sample_log(),
        );

        // Assert
        assert!(script.sudo());
        assert_eq!(script.runas(), Some("deploy"));
    }

    #[test]
    fn build_script_forwards_runas_without_sudo() {
        // Arrange — completes the four-way matrix: runas=Some, sudo=false.
        // Under the script::build_command switch this yields `su <user> -c <script>`
        // with no sudo wrapper.
        let yaml = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/install.sh
      runas: deploy
";
        let (_dir, executor) = build_test_executor(yaml);
        let script_info = &executor.app_spec.as_ref().unwrap().hooks().get("AfterInstall")[0];

        // Act
        let script = executor.build_script(
            script_info,
            PathBuf::from("/archive/scripts/install.sh"),
            &sample_log(),
        );

        // Assert — runas is propagated, sudo defaults to false.
        assert_eq!(script.runas(), Some("deploy"));
        assert!(!script.sudo());
    }

    /// With `reject_path_traversal` on, a hook whose `location` escapes the
    /// archive is rejected without executing the target (planted as a real
    /// executable, so the containment check — not a missing file — stops it).
    #[cfg(unix)]
    #[test]
    fn execute_rejects_hook_location_escaping_archive_when_enabled() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        // ../outside/evil.sh from <dir>/deployment-archive resolves to
        // <dir>/outside/evil.sh — outside the archive.
        let appspec = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: ../outside/evil.sh
      timeout: 10
";
        setup_appspec(dir.path(), appspec);

        let outside = dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let marker = dir.path().join("PWNED");
        std::fs::write(outside.join("evil.sh"), format!("#!/bin/sh\ntouch {}\n", marker.display()))
            .unwrap();
        std::fs::set_permissions(outside.join("evil.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();

        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap()
        .with_reject_path_traversal(true);

        let err = he.execute().unwrap_err();
        assert_eq!(err.error_code, ErrorCode::ScriptMissing);
        assert!(
            err.message.contains("outside the deployment archive"),
            "unexpected message: {}",
            err.message
        );
        assert!(!marker.exists(), "escaping hook must NOT have executed");
    }

    /// Regression: an escaping `location` whose target does NOT exist must
    /// still be rejected as out-of-archive. The earlier check canonicalized
    /// the script path and fell back to the raw (un-normalized) path on
    /// failure — and `canonicalize` fails for a nonexistent target — so
    /// `starts_with` compared `<archive>/../outside/nope.sh` against
    /// `<archive>` and spuriously passed containment (fail-open). Lexical
    /// normalization rejects it regardless of existence.
    #[cfg(unix)]
    #[test]
    fn execute_rejects_nonexistent_escaping_hook_location_when_enabled() {
        let dir = TempDir::new().unwrap();
        let appspec = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: ../outside/nope.sh
      timeout: 10
";
        setup_appspec(dir.path(), appspec);
        // Note: no file planted at ../outside/nope.sh — the escape target does
        // not exist, which is exactly the case the old fall-open missed.

        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap()
        .with_reject_path_traversal(true);

        let err = he.execute().unwrap_err();
        assert_eq!(err.error_code, ErrorCode::ScriptMissing);
        assert!(
            err.message.contains("outside the deployment archive"),
            "escaping location must be rejected as out-of-archive, not merely missing: {}",
            err.message
        );
    }

    /// With `reject_path_traversal` off (default), the containment check does
    /// not fire: an escaping location surfaces as the normal "does not exist"
    /// `ScriptMissing`, not the containment message.
    #[cfg(unix)]
    #[test]
    fn execute_does_not_check_hook_location_containment_when_disabled() {
        let dir = TempDir::new().unwrap();
        let appspec = r"
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: ../../../outside/evil.sh
      timeout: 10
";
        setup_appspec(dir.path(), appspec);

        let spec = s3_spec();
        let he = LifecycleEventExecutor::new(
            LifecycleEventType::AfterInstall,
            &spec,
            dir.path(),
            None,
            None,
        )
        .unwrap(); // reject_path_traversal defaults to false

        let err = he.execute().unwrap_err();
        assert_eq!(err.error_code, ErrorCode::ScriptMissing);
        assert!(
            err.message.contains("does not exist"),
            "with the flag off, the containment check must not fire; got: {}",
            err.message
        );
    }
}
