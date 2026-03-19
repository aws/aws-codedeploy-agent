//! @risk medium
//!
//! Agent configuration.
//!
//! Agent configuration — YAML-based config file parsing and defaults.
//!
//! Loads YAML from `/etc/codedeploy-agent/conf/codedeployagent.yml` and applies
//! Provides typed defaults for all configuration fields.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Default config file path.
pub const DEFAULT_CONFIG_PATH: &str = "/etc/codedeploy-agent/conf/codedeployagent.yml";

/// Default on-premises config file path.
pub const DEFAULT_ON_PREMISES_CONFIG_PATH: &str =
    "/etc/codedeploy-agent/conf/codedeploy.onpremises.yml";

/// Regions where FIPS mode is allowed.
/// Regions where FIPS endpoints are required.
const FIPS_ENABLED_REGIONS: &[&str] = &[
    "us-east-1",
    "us-east-2",
    "us-west-1",
    "us-west-2",
    "us-gov-west-1",
    "us-gov-east-1",
];

/// Agent configuration.
///
/// Fields use `Option` where the YAML file may omit them; [`Default`] provides
/// typed defaults for every field.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct AgentConfig {
    /// Program name. `"codedeploy-agent"`.
    pub program_name: String,

    /// Log directory. `nil` (set by init script to `/var/log/aws/codedeploy-agent`).
    /// Rust default: `/var/log/aws/codedeploy-agent` (baked in since we use systemd, not init script).
    pub log_dir: PathBuf,

    /// PID file directory. derived from `root_dir` at runtime.
    /// Rust default: `/opt/codedeploy-agent/state/.pid` (matches typical deployed path).
    pub pid_dir: PathBuf,

    /// Verbose logging. `false`.
    pub verbose: bool,

    /// Seconds between polling runs. `30`.
    pub wait_between_runs: u64,

    /// Seconds to wait after an error. `30`.
    pub wait_after_error: u64,

    /// HTTP read timeout in seconds. `80`.
    pub http_read_timeout: u64,

    /// Max wait time for graceful shutdown in seconds. `7200`.
    pub kill_agent_max_wait_time_seconds: u64,

    /// Maximum deployment revisions to keep. `5`.
    pub max_revisions: u32,

    /// On-premises config file path.
    pub on_premises_config_file: PathBuf,

    /// HTTP proxy URI. `None`.
    pub proxy_uri: Option<String>,

    /// Enable FIPS endpoints. `false`.
    pub use_fips_mode: bool,

    /// Enable auth policy. `false`.
    pub enable_auth_policy: bool,

    /// Enable deployments log. `true`.
    pub enable_deployments_log: bool,

    /// Deployment root directory.
    pub root_dir: PathBuf,

    /// Deployment state tracking subdirectory name. `"ongoing-deployment"`.
    pub ongoing_deployment_tracking: String,

    /// Custom `CodeDeploy` control endpoint. `None`.
    pub deploy_control_endpoint: Option<String>,

    /// Custom S3 endpoint override. `None`.
    pub s3_endpoint_override: Option<String>,

    /// Disable `IMDSv1` fallback. Default: `false`.
    /// When `true`, only `IMDSv2` (token-based) is used for metadata requests.
    pub disable_imds_v1: bool,

    /// Enable the local command port for debugging. Default: `false`.
    pub enable_command_port: bool,
}

impl Default for AgentConfig {
    /// Defaults for all configuration fields.
    fn default() -> Self {
        Self {
            program_name: "codedeploy-agent".to_string(),
            log_dir: PathBuf::from("/var/log/aws/codedeploy-agent"),
            pid_dir: PathBuf::from("/opt/codedeploy-agent/state/.pid"),
            verbose: false,
            wait_between_runs: 30,
            wait_after_error: 30,
            http_read_timeout: 80,
            kill_agent_max_wait_time_seconds: 7200,
            max_revisions: 5,
            on_premises_config_file: PathBuf::from(DEFAULT_ON_PREMISES_CONFIG_PATH),
            proxy_uri: None,
            use_fips_mode: false,
            enable_auth_policy: false,
            enable_deployments_log: true,
            root_dir: PathBuf::from("/opt/codedeploy-agent/deployment-root"),
            ongoing_deployment_tracking: "ongoing-deployment".to_string(),
            deploy_control_endpoint: None,
            s3_endpoint_override: None,
            disable_imds_v1: false,
            enable_command_port: false,
        }
    }
}

/// Configuration errors.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Failed to read config file.
    #[error("failed to read config file {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// Failed to parse YAML.
    #[error("failed to parse config file {path}: {source}")]
    Parse {
        path: PathBuf,
        source: serde_yaml::Error,
    },
    /// FIPS mode enabled in unsupported region.
    #[error("use_fips_mode can only be enabled in US regions, got {region:?}")]
    FipsRegionNotAllowed { region: String },
    /// No region source available.
    #[error("could not determine AWS region from environment or IMDS")]
    RegionNotFound,
}

impl AgentConfig {
    /// Load config from a YAML file, falling back to defaults for missing fields.
    ///
    /// `ProcessManager::Config.load_config` reads YAML and merges into defaults.
    ///
    /// # Errors
    /// Returns [`ConfigError::Read`] if the file can't be read, or
    /// [`ConfigError::Parse`] if the YAML is malformed.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
        Self::from_yaml(&contents, path)
    }

    /// Load config from a YAML string.
    fn from_yaml(yaml: &str, path: &Path) -> Result<Self, ConfigError> {
        serde_yaml::from_str(yaml)
            .map_err(|source| ConfigError::Parse { path: path.to_path_buf(), source })
    }

    /// Load config from the given path, or the default path, or return defaults.
    ///
    /// Ruby: `--config-file` flag sets `config[:config_file]`, then
    /// `ProcessManager::Config.load_config` reads from that path.
    ///
    /// # Errors
    /// Returns an error if the file exists but can't be read or parsed.
    /// When an explicit path is given and the file doesn't exist, returns
    /// [`ConfigError::Read`] (unlike the default path which silently falls back).
    pub fn load(config_path: Option<&Path>) -> Result<Self, ConfigError> {
        if let Some(p) = config_path {
            Self::from_file(p)
        } else {
            let path = Path::new(DEFAULT_CONFIG_PATH);
            if path.exists() {
                Self::from_file(path)
            } else {
                Ok(Self::default())
            }
        }
    }

    /// Validate FIPS mode against the given region.
    ///
    /// `InstanceAgent::Config#validate_use_fips_mode`
    ///
    /// # Errors
    /// Returns [`ConfigError::FipsRegionNotAllowed`] if FIPS is enabled in a non-US region.
    pub fn validate_fips(&self, region: &str) -> Result<(), ConfigError> {
        if self.use_fips_mode && !FIPS_ENABLED_REGIONS.contains(&region) {
            return Err(ConfigError::FipsRegionNotAllowed { region: region.to_string() });
        }
        Ok(())
    }

    /// Ensure required directories exist, creating them if necessary.
    ///
    /// `ProcessManager::Config#validate_log_and_pid_dir` calls
    /// `FileUtils.mkdir_p` for `log_dir` and `pid_dir`.
    ///
    /// Log directory creation is best-effort (may require root); PID and
    /// state directories are required for the agent to function.
    ///
    /// # Errors
    /// Returns an error if PID or state directories cannot be created.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.pid_dir)?;
        std::fs::create_dir_all(self.root_dir.join(&self.ongoing_deployment_tracking))?;
        // Best-effort: log dir may be under /var/log which requires root.
        let _ = std::fs::create_dir_all(&self.log_dir);
        Ok(())
    }
}

/// Resolve the AWS region using the standard fallback chain.
///
/// `InstanceAgent::Config#region` → `ENV['AWS_REGION'] || InstanceMetadata.region`
///
/// Chain:
/// 1. `AWS_REGION` environment variable
/// 2. IMDS identity document (`/latest/dynamic/instance-identity/document`)
///
/// Uses [`reqwest::blocking`] for the IMDS call with `IMDSv2` (token-based) preferred,
/// falling back to `IMDSv1`, matching the standard IMDS fallback.
///
/// # Errors
/// Returns [`ConfigError::RegionNotFound`] if no region source succeeds.
pub fn resolve_region(disable_imds_v1: bool) -> Result<String, ConfigError> {
    // Step 1: ENV['AWS_REGION']
    if let Ok(region) = std::env::var("AWS_REGION")
        && !region.is_empty()
    {
        return Ok(region);
    }

    // Step 2: IMDS identity document
    if let Some(region) = imds_region(disable_imds_v1) {
        return Ok(region);
    }

    Err(ConfigError::RegionNotFound)
}

/// IMDS endpoint constants.
const IMDS_ENDPOINT: &str = "http://169.254.169.254";
const IMDS_TOKEN_PATH: &str = "/latest/api/token";
const IMDS_IDENTITY_DOC_PATH: &str = "/latest/dynamic/instance-identity/document";
/// `HTTP_TIMEOUT = 10`
const IMDS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Fetch region from IMDS identity document.
///
/// `InstanceMetadata.region` → `identity_document()['region']`
/// Tries `IMDSv2` first (PUT for token, then GET with token), falls back to `IMDSv1`.
fn imds_region(disable_v1: bool) -> Option<String> {
    let client = reqwest::blocking::Client::builder().timeout(IMDS_TIMEOUT).build().ok()?;

    let body = imds_get_identity_doc(&client, disable_v1)?;
    parse_region_from_identity_doc(&body)
}

/// Fetch the identity document from IMDS, trying v2 then v1.
///
/// `InstanceMetadata.get_instance_metadata` tries PUT for token first,
/// rescues and falls back to plain GET.
fn imds_get_identity_doc(client: &reqwest::blocking::Client, disable_v1: bool) -> Option<String> {
    let url = format!("{IMDS_ENDPOINT}{IMDS_IDENTITY_DOC_PATH}");

    // IMDSv2: get token, then GET with token
    if let Some(body) = imds_v2_get(client, &url) {
        return Some(body);
    }

    if disable_v1 {
        return None;
    }

    // IMDSv1 fallback: plain GET
    client.get(&url).send().ok().and_then(|r| r.text().ok())
}

/// `IMDSv2`: PUT for token, GET with token header.
fn imds_v2_get(client: &reqwest::blocking::Client, url: &str) -> Option<String> {
    let token_url = format!("{IMDS_ENDPOINT}{IMDS_TOKEN_PATH}");
    // request['X-aws-ec2-metadata-token-ttl-seconds'] = '21600'
    let token = client
        .put(&token_url)
        .header("X-aws-ec2-metadata-token-ttl-seconds", "21600")
        .send()
        .ok()?
        .text()
        .ok()?;

    client
        .get(url)
        .header("X-aws-ec2-metadata-token", &token)
        .send()
        .ok()?
        .text()
        .ok()
}

/// Parse `region` from the IMDS identity document JSON.
///
/// `JSON.parse(body)['region'].strip`
fn parse_region_from_identity_doc(body: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(body).ok()?;
    doc.get("region")?.as_str().map(|s| s.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_matches_ruby_program_name() {
        let config = AgentConfig::default();
        assert_eq!(config.program_name, "codedeploy-agent");
    }

    #[test]
    fn default_matches_ruby_wait_between_runs() {
        let config = AgentConfig::default();
        assert_eq!(config.wait_between_runs, 30);
    }

    #[test]
    fn default_matches_ruby_http_read_timeout() {
        let config = AgentConfig::default();
        assert_eq!(config.http_read_timeout, 80);
    }

    #[test]
    fn default_matches_ruby_kill_agent_max_wait() {
        let config = AgentConfig::default();
        assert_eq!(config.kill_agent_max_wait_time_seconds, 7200);
    }

    #[test]
    fn default_matches_ruby_max_revisions() {
        let config = AgentConfig::default();
        assert_eq!(config.max_revisions, 5);
    }

    #[test]
    fn default_matches_ruby_on_premises_path() {
        let config = AgentConfig::default();
        assert_eq!(
            config.on_premises_config_file,
            PathBuf::from("/etc/codedeploy-agent/conf/codedeploy.onpremises.yml")
        );
    }

    #[test]
    fn default_fips_mode_is_false() {
        assert!(!AgentConfig::default().use_fips_mode);
    }

    #[test]
    fn default_verbose_is_false() {
        assert!(!AgentConfig::default().verbose);
    }

    #[test]
    fn default_enable_deployments_log_is_true() {
        assert!(AgentConfig::default().enable_deployments_log);
    }

    #[test]
    fn default_enable_auth_policy_is_false() {
        assert!(!AgentConfig::default().enable_auth_policy);
    }

    #[test]
    fn default_proxy_uri_is_none() {
        assert!(AgentConfig::default().proxy_uri.is_none());
    }

    #[test]
    fn default_deploy_control_endpoint_is_none() {
        assert!(AgentConfig::default().deploy_control_endpoint.is_none());
    }

    #[test]
    fn default_s3_endpoint_override_is_none() {
        assert!(AgentConfig::default().s3_endpoint_override.is_none());
    }

    #[test]
    fn default_disable_imds_v1_is_false() {
        assert!(!AgentConfig::default().disable_imds_v1);
    }

    #[test]
    fn default_enable_command_port_is_false() {
        assert!(!AgentConfig::default().enable_command_port);
    }

    #[test]
    fn from_yaml_empty_returns_defaults() {
        let config = AgentConfig::from_yaml("{}", Path::new("test.yml")).unwrap();
        assert_eq!(config.wait_between_runs, 30);
        assert_eq!(config.program_name, "codedeploy-agent");
    }

    #[test]
    fn from_yaml_overrides_fields() {
        let yaml = r#"
wait_between_runs: 10
verbose: true
proxy_uri: "http://proxy:8080"
max_revisions: 3
"#;
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.wait_between_runs, 10);
        assert!(config.verbose);
        assert_eq!(config.proxy_uri.as_deref(), Some("http://proxy:8080"));
        assert_eq!(config.max_revisions, 3);
        // Unset fields keep defaults
        assert_eq!(config.http_read_timeout, 80);
    }

    #[test]
    fn from_yaml_invalid_yaml_returns_parse_error() {
        let err = AgentConfig::from_yaml("{{{", Path::new("bad.yml")).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }));
        assert!(err.to_string().contains("bad.yml"));
    }

    #[test]
    fn from_file_missing_returns_read_error() {
        let err = AgentConfig::from_file(Path::new("/nonexistent/config.yml")).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }));
    }

    #[test]
    fn from_file_reads_yaml() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.yml");
        std::fs::write(&path, "wait_between_runs: 15\nverbose: true\n").unwrap();
        let config = AgentConfig::from_file(&path).unwrap();
        assert_eq!(config.wait_between_runs, 15);
        assert!(config.verbose);
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        // DEFAULT_CONFIG_PATH won't exist in test environment
        let config = AgentConfig::load(None).unwrap();
        assert_eq!(config.wait_between_runs, 30);
    }

    #[test]
    fn validate_fips_allows_us_regions() {
        let mut config = AgentConfig::default();
        config.use_fips_mode = true;
        assert!(config.validate_fips("us-east-1").is_ok());
        assert!(config.validate_fips("us-west-2").is_ok());
        assert!(config.validate_fips("us-gov-west-1").is_ok());
        assert!(config.validate_fips("us-gov-east-1").is_ok());
    }

    #[test]
    fn validate_fips_rejects_non_us_regions() {
        let mut config = AgentConfig::default();
        config.use_fips_mode = true;
        let err = config.validate_fips("eu-west-1").unwrap_err();
        assert!(matches!(err, ConfigError::FipsRegionNotAllowed { .. }));
        assert!(err.to_string().contains("eu-west-1"));
    }

    #[test]
    fn validate_fips_skips_check_when_disabled() {
        let config = AgentConfig::default();
        // FIPS disabled, any region is fine
        assert!(config.validate_fips("ap-southeast-1").is_ok());
    }

    #[test]
    fn config_is_cloneable() {
        let config = AgentConfig::default();
        let cloned = config.clone();
        assert_eq!(config.program_name, cloned.program_name);
    }

    #[test]
    fn config_is_debuggable() {
        let config = AgentConfig::default();
        let debug = format!("{config:?}");
        assert!(debug.contains("codedeploy-agent"));
    }

    #[test]
    fn from_yaml_with_all_fields() {
        let yaml = r#"
program_name: "my-agent"
log_dir: "/tmp/logs"
pid_dir: "/tmp/pids"
verbose: true
wait_between_runs: 5
wait_after_error: 10
http_read_timeout: 120
kill_agent_max_wait_time_seconds: 3600
max_revisions: 10
on_premises_config_file: "/custom/onprem.yml"
proxy_uri: "http://proxy:3128"
use_fips_mode: true
enable_auth_policy: true
enable_deployments_log: false
root_dir: "/custom/root"
ongoing_deployment_tracking: "custom-tracking"
deploy_control_endpoint: "https://custom.endpoint"
s3_endpoint_override: "https://s3.custom"
disable_imds_v1: true
enable_command_port: true
"#;
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.program_name, "my-agent");
        assert_eq!(config.log_dir, PathBuf::from("/tmp/logs"));
        assert_eq!(config.pid_dir, PathBuf::from("/tmp/pids"));
        assert!(config.verbose);
        assert_eq!(config.wait_between_runs, 5);
        assert_eq!(config.wait_after_error, 10);
        assert_eq!(config.http_read_timeout, 120);
        assert_eq!(config.kill_agent_max_wait_time_seconds, 3600);
        assert_eq!(config.max_revisions, 10);
        assert_eq!(config.on_premises_config_file, PathBuf::from("/custom/onprem.yml"));
        assert_eq!(config.proxy_uri.as_deref(), Some("http://proxy:3128"));
        assert!(config.use_fips_mode);
        assert!(config.enable_auth_policy);
        assert!(!config.enable_deployments_log);
        assert_eq!(config.root_dir, PathBuf::from("/custom/root"));
        assert_eq!(config.ongoing_deployment_tracking, "custom-tracking");
        assert_eq!(config.deploy_control_endpoint.as_deref(), Some("https://custom.endpoint"));
        assert_eq!(config.s3_endpoint_override.as_deref(), Some("https://s3.custom"));
        assert!(config.disable_imds_v1);
        assert!(config.enable_command_port);
    }

    #[test]
    fn from_yaml_ignores_unknown_fields() {
        let yaml = "unknown_key: some_value\nwait_between_runs: 42\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.wait_between_runs, 42);
    }

    #[test]
    fn load_with_explicit_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("custom.yml");
        std::fs::write(&path, "wait_between_runs: 99\n").unwrap();
        let config = AgentConfig::load(Some(&path)).unwrap();
        assert_eq!(config.wait_between_runs, 99);
    }

    #[test]
    fn load_with_explicit_nonexistent_path_returns_error() {
        let err = AgentConfig::load(Some(Path::new("/no/such/file.yml"))).unwrap_err();
        assert!(matches!(err, ConfigError::Read { .. }));
    }

    #[test]
    fn parse_region_from_identity_doc_extracts_region() {
        let doc = r#"{"region": "us-west-2", "accountId": "123456789"}"#;
        assert_eq!(parse_region_from_identity_doc(doc).as_deref(), Some("us-west-2"));
    }

    #[test]
    fn parse_region_from_identity_doc_trims_whitespace() {
        let doc = r#"{"region": "  eu-west-1  "}"#;
        assert_eq!(parse_region_from_identity_doc(doc).as_deref(), Some("eu-west-1"));
    }

    #[test]
    fn parse_region_from_identity_doc_returns_none_for_missing_key() {
        let doc = r#"{"accountId": "123"}"#;
        assert!(parse_region_from_identity_doc(doc).is_none());
    }

    #[test]
    fn parse_region_from_identity_doc_returns_none_for_invalid_json() {
        assert!(parse_region_from_identity_doc("{invalid").is_none());
    }

    #[test]
    fn resolve_region_uses_env_var_when_set() {
        // Can't mutate env vars with #![forbid(unsafe_code)] in Rust 2024 edition.
        // We can only verify the env var path when AWS_REGION is already set.
        // The IMDS path and parsing logic are tested separately.
        if let Ok(region) = std::env::var("AWS_REGION") {
            if !region.is_empty() {
                assert_eq!(resolve_region(false).unwrap(), region);
            }
        }
    }

    #[test]
    fn resolve_region_error_is_descriptive() {
        let err = ConfigError::RegionNotFound;
        assert!(err.to_string().contains("could not determine AWS region"));
    }

    #[test]
    fn ensure_dirs_creates_missing_directories() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = AgentConfig::default();
        config.pid_dir = dir.path().join("pid");
        config.log_dir = dir.path().join("logs");
        config.root_dir = dir.path().join("root");
        config.ongoing_deployment_tracking = "ongoing".to_string();

        config.ensure_dirs().unwrap();

        assert!(config.pid_dir.is_dir());
        assert!(config.log_dir.is_dir());
        assert!(config.root_dir.join("ongoing").is_dir());
    }

    #[test]
    fn ensure_dirs_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = AgentConfig::default();
        config.pid_dir = dir.path().join("pid");
        config.log_dir = dir.path().join("logs");
        config.root_dir = dir.path().join("root");

        config.ensure_dirs().unwrap();
        config.ensure_dirs().unwrap(); // second call should not fail
    }
}
