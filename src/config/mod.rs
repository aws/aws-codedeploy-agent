//! Agent configuration — YAML-based config file parsing and defaults.
//!
//! Loads YAML from `/etc/codedeploy-agent/conf/codedeployagent.yml` and merges
//! it over typed defaults for every configuration field.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::paths;

/// Default config file path.
///
/// Returns the platform-appropriate path via [`paths::config_file`].
#[must_use]
pub fn default_config_path() -> PathBuf {
    paths::config_file()
}

/// Default on-premises config file path.
///
/// Returns the platform-appropriate path via [`paths::on_premises_config_file`].
#[must_use]
pub fn default_on_premises_config_path() -> PathBuf {
    paths::on_premises_config_file()
}

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

/// Maximum allowed value for `kill_agent_max_wait_time_seconds` (24 hours).
///
/// A misconfigured value (e.g. `999999999`) would prevent the agent from
/// being killed during a stuck deployment, leaving the host in a degraded
/// state for years. 24 hours is generous enough for any legitimate hook
/// while bounding the `DoS` window.
pub const MAX_KILL_AGENT_WAIT_SECONDS: u64 = 86400;

/// Deserialize an optional byte size that accepts either a raw integer or a
/// human-readable string like `"4GB"`, `"500MB"`, `"1GiB"`, `"2048KB"`.
///
/// Supported suffixes (case-insensitive): B, KB, MB, GB, TB, KiB, MiB, GiB, TiB.
/// SI units use powers of 1000; binary units use powers of 1024.
fn deserialize_byte_size_option<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::Error;

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum ByteSizeValue {
        Int(u64),
        Str(String),
    }

    let Some(val) = Option::<ByteSizeValue>::deserialize(deserializer)? else {
        return Ok(None);
    };

    match val {
        ByteSizeValue::Int(n) => Ok(Some(n)),
        ByteSizeValue::Str(s) => parse_byte_size(&s).map(Some).map_err(Error::custom),
    }
}

/// Parse a human-readable byte size string into bytes.
///
/// Accepts integer values with optional suffix. Fractional values (e.g. "1.5GB")
/// are rejected — use a smaller unit instead ("1500MB").
fn parse_byte_size(s: &str) -> Result<u64, String> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num_str, suffix) = s.split_at(split);
    let num: u64 = num_str.trim().parse().map_err(|_| format!("invalid byte size: {s}"))?;
    let multiplier: u64 = match suffix.trim().to_lowercase().as_str() {
        "" | "b" => 1,
        "kb" => 1_000,
        "mb" => 1_000_000,
        "gb" => 1_000_000_000,
        "tb" => 1_000_000_000_000,
        "kib" => 1_024,
        "mib" => 1_048_576,
        "gib" => 1_073_741_824,
        "tib" => 1_099_511_627_776,
        other => return Err(format!("unknown size suffix: {other}")),
    };
    num.checked_mul(multiplier)
        .ok_or_else(|| format!("byte size out of range: {s}"))
}

/// Strip legacy symbol-style leading-colon keys from YAML lines.
///
/// Earlier agent versions serialised config keys as YAML symbols
/// (`:key: value`). This pre-processes the raw YAML so that `serde_yaml` can map
/// them to struct fields without requiring customers to rewrite their configs.
pub(crate) fn strip_symbol_keys(yaml: &str) -> String {
    let mut out = String::with_capacity(yaml.len());
    for (i, line) in yaml.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let trimmed = line.trim_start();
        if trimmed.starts_with(':') && trimmed.len() > 1 && trimmed.as_bytes()[1] != b' ' {
            let indent = &line[..line.len() - trimmed.len()];
            out.push_str(indent);
            out.push_str(&trimmed[1..]);
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Opt-in security-hardening toggles. Every field defaults to `false`, which
/// preserves the historical, backwards-compatible behavior; setting one to
/// `true` enables the stricter behavior. Grouped into their own struct so a new
/// flag is a one-line addition, but `#[serde(flatten)]`'d into `AgentConfig` so
/// the YAML file keeps every key at the top level — the grouping is not visible
/// to customers.
///
/// `#[serde(default)]` on the struct makes each absent key default to `false`,
/// which is required because these keys are omitted from almost every real
/// config file.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(default)]
#[allow(clippy::struct_excessive_bools)]
pub struct HardeningConfig {
    /// Reject bundles containing symlinks or hardlinks during extraction.
    /// Default: `false` (symlinks allowed, the backwards-compatible behavior).
    /// When `true`, any symlink or hardlink in the extracted archive causes
    /// the deployment to fail before files are used.
    pub reject_symlinks_in_bundle: bool,

    /// Reset ownership of extracted bundle files to the extracting process
    /// (root) instead of applying the tar header uid/gid. Default `false`:
    /// root tar's `--same-owner` default applies the bundle-builder's uid, and
    /// deployments may rely on the resulting non-root write access inside the
    /// deployment archive.
    ///
    /// When `true`, extraction passes `--no-same-owner` (system tar) /
    /// disables ownership preservation (native fallback), so a local user whose
    /// uid collides with the bundle-builder's can no longer modify extracted
    /// hook scripts before the agent executes them as root. Zip bundles are
    /// unaffected either way — `unzip` never applies archive ownership (no
    /// `-X`).
    pub ignore_ownership_in_bundle: bool,

    /// Reject `AppSpec` `SELinux` types `unconfined_t`, `kernel_t`, `init_t`.
    /// Default `false` (backwards-compatible: such types are accepted).
    pub reject_unconfined_selinux_in_bundle: bool,

    /// Reject SUID/SGID bits in extracted bundle files and `AppSpec` modes.
    /// Default `false` (backwards-compatible: such bits are accepted).
    pub reject_unsafe_permissions_in_bundle: bool,

    /// Reject `..`/absolute-path traversal in archive entries, the `AppSpec`
    /// `files.source` field, and the `AppSpec` `hooks.location` field — each
    /// rejected if it resolves outside the deployment archive. Default `false`
    /// (backwards-compatible: such paths are accepted).
    pub reject_path_traversal_in_bundle: bool,

    /// Reject an `AppSpec` `permissions:` target (chmod/chown/setfacl/semanage)
    /// whose destination is a symlink, and apply the permission via no-follow
    /// syscalls. Default `false` (backwards-compatible: permission sinks follow
    /// a symlinked destination).
    ///
    /// By default each permission sink uses symlink-*following* operations
    /// (chown/chmod on the path, `setfacl --set`, `semanage`/`restorecon` on the
    /// path). A local actor who can write into the deployment archive (the
    /// documented `permissions.owner: <service-user>` privilege-drop pattern
    /// leaves the tree owned by that non-root user for the whole Install phase)
    /// can swap a freshly-copied file for a symlink to e.g. `/etc/passwd` between
    /// the copy and the permission op; the root agent then follows the link and
    /// re-owns/relabels the target (CWE-59 / CWE-367 TOCTOU local privilege
    /// escalation).
    ///
    /// When `true`, each permission sink first rejects a symlinked destination
    /// (`lstat`) and then uses the no-follow syscall (`lchown`; `O_NOFOLLOW` +
    /// `fchmod`; `setfacl --physical`; a device+inode swap check before
    /// `semanage`). A deployment whose `permissions:` block deliberately targets
    /// a symlink and relied on the op reaching through to the link target fails
    /// with `SymlinkDestinationRejected` (reference the real target instead).
    ///
    /// Left off by default because such a bundle deployed successfully on
    /// earlier agent versions, so an unconditional reject would break it on
    /// upgrade. The write-side symlink race — the actual escalation vector — is
    /// closed **unconditionally** by the `O_NOFOLLOW` copy in `copy_command.rs`
    /// regardless of this flag; this flag only governs the
    /// permission-application sinks.
    pub reject_symlink_permission_targets: bool,

    /// Restrict everything under the agent's install root
    /// (`/opt/codedeploy-agent` by default) to root-only modes instead of the
    /// backwards-compatible world-readable ones. Default `false`.
    ///
    /// These are world-readable (umask-derived 0755/0644) by default because
    /// host tooling outside the agent depends on reading them (e.g. reading
    /// `ongoing-deployment` for restart telemetry). The tightened modes are
    /// opt-in hardening for hosts where no third-party process reads agent
    /// state. Scope:
    /// - dirs: `root_dir`, `ongoing-deployment`, `deployment-instructions`,
    ///   per-group/per-deployment dirs, `deployment-archive`, `pid_dir`
    ///   (hardened 0700, or 0711 where `runas:` traversal is needed;
    ///   default 0755)
    /// - per-deployment logs: `deployment-logs/` +
    ///   `<program>-deployments.log`, each deployment's `logs/scripts.log`
    ///   (hardened 0750/0640; default 0755/0644)
    /// - agent state files: PID file, deployment tracking files,
    ///   `install.json`/cleanup instruction files, last-successful /
    ///   most-recent markers, bundle `ETag` marker, downloaded bundles
    ///   (hardened 0600; default 0644)
    ///
    /// The agent log directory (`log_dir`, outside the install root) is
    /// governed by `restrict_log_dir_permissions` instead.
    pub restrict_agent_dir_permissions: bool,

    /// Restrict the agent log directory (`log_dir`, default
    /// `/var/log/aws/codedeploy-agent`) to root+group-only modes (dir 0750,
    /// agent/updater log files 0640) instead of the backwards-compatible
    /// world-readable 0755/0644. Default `false`.
    ///
    /// WARNING: with this set, **non-root** log collectors (`CloudWatch`
    /// agent, fluentd, Datadog, …) lose access to the agent and updater
    /// logs. Enable only where log shipping runs as root or is not needed.
    pub restrict_log_dir_permissions: bool,

    /// Strip `LD_PRELOAD`/`LD_LIBRARY_PATH`/`LD_AUDIT` from the hook environment (Unix). Default `false`.
    pub strip_loader_env_in_hooks: bool,

    /// Restrict the hook environment to shell basics plus deployment vars instead of full inheritance. Default `false`.
    pub restrict_hook_env_to_allowlist: bool,

    /// Run `.ps1` lifecycle hooks with `-NoProfile -NonInteractive` flags
    /// (Windows only). Default `false` — the backwards-compatible behavior, in
    /// which PowerShell hooks load the user's profile and allow interactive
    /// prompts because only `-ExecutionPolicy Bypass -File` is passed.
    ///
    /// When `true`, the added flags prevent profile scripts from injecting
    /// code and suppress interactive prompts that would hang a service-context
    /// agent — defense-in-depth hardening for Windows hooks.
    pub disable_powershell_profile_in_hooks: bool,
}

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

    /// Log directory. Default: `/var/log/aws/codedeploy-agent`.
    pub log_dir: PathBuf,

    /// PID file directory. Default: `/opt/codedeploy-agent/state/.pid`.
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

    /// Capture Amazon S3 HTTP wire logs to `<program_name>.aws_wire.log` in
    /// `log_dir`. Default: `false`.
    ///
    /// When `true`, every S3 request/response (method, URI, status, headers) is
    /// appended to a dedicated, size-rotated wire log. Corresponds to the
    /// `:log_aws_wire:` setting in the published `codedeployagent.yml` docs.
    ///
    /// SECURITY: the wire log can contain sensitive data (the docs warn it may
    /// hold plaintext contents of transferred objects and covers all S3 activity
    /// for the account), so — unlike the world-readable agent log — it is written
    /// with restrictive `0640` permissions. Enable only while actively debugging;
    /// the file can grow very large very quickly.
    pub log_aws_wire: bool,

    /// Suppress core dumps at startup. Default: `true`.
    pub disable_core_dumps: bool,

    /// Opt-in hardening toggles (all default `false`, preserving
    /// backwards-compatible behavior). Flattened into the top-level YAML
    /// namespace via `#[serde(flatten)]`, so customers still write e.g.
    /// `reject_path_traversal_in_bundle: true` at the top level — the grouping
    /// is internal only and does not change the config file format.
    #[serde(flatten)]
    pub hardening: HardeningConfig,

    /// Maximum total declared extraction size in bytes. `None` (the default) = no cap.
    /// When set, archives declaring more than this total uncompressed size are rejected
    /// before extraction.
    /// Accepts raw bytes (integer) or human-readable strings: `"4GB"`, `"500MB"`, `"1GiB"`.
    #[serde(default, deserialize_with = "deserialize_byte_size_option")]
    pub archive_max_extraction_size: Option<u64>,

    /// Catch-all for keys not matched by any field above. serde routes unknown
    /// keys here (so the struct is the single source of truth for what's known);
    /// they are ignored at runtime and logged once, which surfaces deprecated or
    /// misspelled options that silently have no effect. Not a real config option.
    #[doc(hidden)]
    #[serde(flatten)]
    pub unknown_keys: std::collections::BTreeMap<String, serde_yaml::Value>,
}

impl Default for AgentConfig {
    /// Defaults for all configuration fields.
    fn default() -> Self {
        Self {
            program_name: "codedeploy-agent".to_string(),
            log_dir: paths::log_dir(),
            pid_dir: paths::pid_dir(),
            verbose: false,
            wait_between_runs: 30,
            wait_after_error: 30,
            http_read_timeout: 80,
            kill_agent_max_wait_time_seconds: 7200,
            max_revisions: 5,
            on_premises_config_file: paths::on_premises_config_file(),
            proxy_uri: None,
            use_fips_mode: false,
            enable_auth_policy: false,
            enable_deployments_log: true,
            root_dir: paths::root_dir(),
            ongoing_deployment_tracking: "ongoing-deployment".to_string(),
            deploy_control_endpoint: None,
            s3_endpoint_override: None,
            disable_imds_v1: false,
            enable_command_port: false,
            log_aws_wire: false,
            disable_core_dumps: true,
            hardening: HardeningConfig::default(),
            archive_max_extraction_size: None,
            unknown_keys: std::collections::BTreeMap::new(),
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
    /// `deploy_control_endpoint` uses a disallowed URL scheme.
    #[error(
        "deploy_control_endpoint in {path} has invalid scheme: {endpoint:?}; \
         only https:// or http:// is accepted"
    )]
    InvalidEndpoint { path: PathBuf, endpoint: String },
    /// No region source available.
    #[error("could not determine AWS region from environment or IMDS")]
    RegionNotFound,
    /// No host identifier source available.
    #[error("could not determine host identifier from environment or IMDS")]
    HostIdentifierNotFound,
}

impl AgentConfig {
    /// Load config from a YAML file, falling back to defaults for missing fields.
    ///
    /// # Errors
    /// Returns [`ConfigError::Read`] if the file can't be read, or
    /// [`ConfigError::Parse`] if the YAML is malformed.
    pub fn from_file(path: &Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)
            .map_err(|source| ConfigError::Read { path: path.to_path_buf(), source })?;
        let mut config = Self::from_yaml(&contents, path)?;
        config.normalize();
        config.validate(path)?;
        Ok(config)
    }

    /// Load config from a YAML string.
    ///
    /// Transparently strips legacy symbol-style leading-colon keys
    /// (`:key: value` → `key: value`) so configs written for earlier agent
    /// versions parse correctly. Unknown keys (e.g. options no longer supported)
    /// are ignored; each is logged once so operators can see it had no effect.
    fn from_yaml(yaml: &str, path: &Path) -> Result<Self, ConfigError> {
        let yaml = strip_symbol_keys(yaml);
        let config: Self = serde_yaml::from_str(&yaml)
            .map_err(|source| ConfigError::Parse { path: path.to_path_buf(), source })?;
        for key in config.unknown_keys.keys() {
            tracing::warn!(
                %key,
                path = %path.display(),
                "ignoring unknown config key (deprecated or unsupported)"
            );
        }
        Ok(config)
    }

    /// Apply post-parse hardening: clamp out-of-range fields to safe values.
    ///
    /// `kill_agent_max_wait_time_seconds` is capped at
    /// [`MAX_KILL_AGENT_WAIT_SECONDS`] so a misconfigured value (e.g. 999999999)
    /// can't strand the host in a stuck-deployment state for years.
    fn normalize(&mut self) {
        if self.kill_agent_max_wait_time_seconds > MAX_KILL_AGENT_WAIT_SECONDS {
            tracing::warn!(
                configured = self.kill_agent_max_wait_time_seconds,
                capped = MAX_KILL_AGENT_WAIT_SECONDS,
                "kill_agent_max_wait_time_seconds exceeds maximum, capping"
            );
            self.kill_agent_max_wait_time_seconds = MAX_KILL_AGENT_WAIT_SECONDS;
        }
    }

    /// Validate post-parse fields that should fail loud rather than be clamped.
    ///
    /// Accepts `https://` and `http://` (the latter with a cleartext `warn!`).
    /// The `http://` case is a deliberate operator choice so we surface it loudly
    /// rather than block it. The garbage schemes (`file://`, `javascript:`,
    /// `ftp://`, `gopher://`, `data:`, etc.) are rejected unconditionally —
    /// no legitimate agent use case.
    fn validate(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(ref endpoint) = self.deploy_control_endpoint {
            if endpoint.starts_with("https://") {
                // OK
            } else if endpoint.starts_with("http://") {
                tracing::warn!(
                    endpoint = %endpoint,
                    "deploy_control_endpoint uses plaintext HTTP; credentials \
                     and command payloads transmit in cleartext"
                );
            } else {
                return Err(ConfigError::InvalidEndpoint {
                    path: path.to_path_buf(),
                    endpoint: endpoint.clone(),
                });
            }
        }
        Ok(())
    }

    /// Load config from the given path, or the default path, or return defaults.
    ///
    /// The `--config-file` flag supplies the explicit path.
    ///
    /// # Errors
    /// Returns an error if the file exists but can't be read or parsed.
    /// When an explicit path is given and the file doesn't exist, returns
    /// [`ConfigError::Read`] (unlike the default path which silently falls back).
    pub fn load(config_path: Option<&Path>) -> Result<Self, ConfigError> {
        if let Some(p) = config_path {
            Self::from_file(p)
        } else {
            let path = default_config_path();
            if path.exists() {
                Self::from_file(&path)
            } else {
                Ok(Self::default())
            }
        }
    }

    /// Validate FIPS mode against the given region.
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
    /// Log directory creation is best-effort (may require root); PID and
    /// state directories are required for the agent to function.
    ///
    /// Install-root dirs (`pid_dir`, `root_dir`, `ongoing-deployment`)
    /// follow `restrict_agent_dir_permissions`: world-readable 0755 by
    /// default (host tooling outside the agent reads these directories),
    /// 0700/0711 when the opt-in hardening flag is set. The log dir follows
    /// `restrict_log_dir_permissions`: 0755 by default (world-readable
    /// agent/updater logs for non-root log collectors), 0750 when set. On
    /// non-Unix, modes are ignored.
    ///
    /// # Errors
    /// Returns an error if PID or state directories cannot be created.
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        self.ensure_deployment_dir(&self.pid_dir, 0o700)?;
        self.ensure_deployment_dir(&self.root_dir, 0o711)?;
        self.ensure_deployment_dir(&self.root_dir.join(&self.ongoing_deployment_tracking), 0o700)?;
        // Best-effort: log dir may be under /var/log which requires root.
        // Policy per `restrict_log_dir_permissions` — 0755 default so
        // non-root log collectors can read the agent/updater logs, 0750
        // hardened. `create_deployment_dir` force-sets the mode in both
        // directions, covering the upgrade path for callers that run
        // `ensure_dirs` without `agent_logger::init` (deploy-local, update).
        let _ = crate::system::create_deployment_dir(
            &self.log_dir,
            0o750,
            self.hardening.restrict_log_dir_permissions,
        );
        Ok(())
    }

    /// Create a deployment-root directory with the mode policy selected by
    /// `restrict_agent_dir_permissions`. See
    /// [`crate::system::create_deployment_dir`].
    pub(crate) fn ensure_deployment_dir(
        &self,
        path: &Path,
        hardened_mode: u32,
    ) -> std::io::Result<()> {
        crate::system::create_deployment_dir(
            path,
            hardened_mode,
            self.hardening.restrict_agent_dir_permissions,
        )
    }
}

/// Resolve the AWS region using the standard fallback chain.
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
/// IMDS request timeout.
const IMDS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Fetch region from IMDS identity document.
///
/// Tries `IMDSv2` first (PUT for token, then GET with token), falls back to `IMDSv1`.
fn imds_region(disable_v1: bool) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(IMDS_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .ok()?;

    let body = imds_get_identity_doc(&client, disable_v1)?;
    parse_region_from_identity_doc(&body)
}

/// Fetch the identity document from IMDS, trying v2 then v1.
///
/// The v2 path PUTs for a token first; on failure it falls back to a plain GET.
fn imds_get_identity_doc(client: &reqwest::blocking::Client, disable_v1: bool) -> Option<String> {
    let url = format!("{IMDS_ENDPOINT}{IMDS_IDENTITY_DOC_PATH}");

    // IMDSv2: get token, then GET with token
    if let Some(body) = imds_v2_get(client, &url) {
        return Some(body);
    }

    if disable_v1 {
        return None;
    }

    // IMDSv1 fallback: plain GET with status check
    imds_v1_get(client, &url)
}

/// `IMDSv2`: PUT for token, GET with token header.
fn imds_v2_get(client: &reqwest::blocking::Client, url: &str) -> Option<String> {
    let token_url = format!("{IMDS_ENDPOINT}{IMDS_TOKEN_PATH}");
    let token = client
        .put(&token_url)
        .header("X-aws-ec2-metadata-token-ttl-seconds", "21600")
        .send()
        .ok()
        .filter(|r| r.status().is_success())?
        .text()
        .ok()?;

    let resp = client
        .get(url)
        .header("X-aws-ec2-metadata-token", &token)
        .send()
        .ok()
        .filter(|r| r.status().is_success())?;
    resp.text().ok()
}

/// `IMDSv1` fallback: plain GET with status check.
fn imds_v1_get(client: &reqwest::blocking::Client, url: &str) -> Option<String> {
    client
        .get(url)
        .send()
        .ok()
        .filter(|r| r.status().is_success())
        .and_then(|r| r.text().ok())
}

/// Parse `region` from the IMDS identity document JSON.
fn parse_region_from_identity_doc(body: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(body).ok()?;
    doc.get("region")?.as_str().map(|s| s.trim().to_string())
}

/// Resolve the EC2 host identifier for `InstanceProfile` mode.
///
/// The identifier is the ARN
/// `arn:{partition}:ec2:{region}:{accountId}:instance/{instanceId}`.
///
/// Chain:
/// 1. `AWS_HOST_IDENTIFIER` environment variable
/// 2. IMDS identity document + partition metadata
///
/// # Errors
/// Returns [`ConfigError::HostIdentifierNotFound`] if no source succeeds.
pub fn resolve_host_identifier(disable_imds_v1: bool) -> Result<String, ConfigError> {
    // Step 1: ENV override
    if let Ok(id) = std::env::var("AWS_HOST_IDENTIFIER")
        && !id.is_empty()
    {
        return Ok(id);
    }

    // Step 2: IMDS identity document + partition
    let client = reqwest::blocking::Client::builder()
        .timeout(IMDS_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ConfigError::HostIdentifierNotFound)?;
    let body = imds_get_identity_doc(&client, disable_imds_v1)
        .ok_or(ConfigError::HostIdentifierNotFound)?;
    let partition = imds_partition(&client, disable_imds_v1);
    parse_host_identifier_from_identity_doc(&body, &partition)
        .ok_or(ConfigError::HostIdentifierNotFound)
}

/// Resolve both region and host identifier from a single IMDS identity document fetch.
///
/// This avoids two separate IMDS identity document requests when both region and
/// host identifier need to be resolved (`InstanceProfile` mode).
///
/// # Errors
/// Returns [`ConfigError::RegionNotFound`] if region cannot be determined.
/// Returns [`ConfigError::HostIdentifierNotFound`] if host identifier cannot be determined.
pub fn resolve_region_and_host_identifier(
    disable_imds_v1: bool,
) -> Result<(String, String), ConfigError> {
    // ENV overrides — check both before hitting IMDS.
    let env_region = std::env::var("AWS_REGION").ok().filter(|s| !s.is_empty());
    let env_host_id = std::env::var("AWS_HOST_IDENTIFIER").ok().filter(|s| !s.is_empty());

    // If both are set via env, skip IMDS entirely.
    if let (Some(region), Some(host_id)) = (&env_region, &env_host_id) {
        return Ok((region.clone(), host_id.clone()));
    }

    // At least one needs IMDS — fetch identity document once.
    let client = reqwest::blocking::Client::builder()
        .timeout(IMDS_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| {
            if env_region.is_some() {
                ConfigError::HostIdentifierNotFound
            } else {
                ConfigError::RegionNotFound
            }
        })?;
    let body = imds_get_identity_doc(&client, disable_imds_v1).ok_or_else(|| {
        if env_region.is_some() {
            ConfigError::HostIdentifierNotFound
        } else {
            ConfigError::RegionNotFound
        }
    })?;

    let region = env_region
        .or_else(|| parse_region_from_identity_doc(&body))
        .ok_or(ConfigError::RegionNotFound)?;

    let host_id = env_host_id
        .or_else(|| {
            let partition = imds_partition(&client, disable_imds_v1);
            parse_host_identifier_from_identity_doc(&body, &partition)
        })
        .ok_or(ConfigError::HostIdentifierNotFound)?;

    Ok((region, host_id))
}

/// IMDS partition metadata path.
const IMDS_PARTITION_PATH: &str = "/latest/meta-data/services/partition";

/// Fetch AWS partition from IMDS, defaulting to `"aws"`.
fn imds_partition(client: &reqwest::blocking::Client, disable_v1: bool) -> String {
    let url = format!("{IMDS_ENDPOINT}{IMDS_PARTITION_PATH}");
    // IMDSv2 first
    if let Some(p) = imds_v2_get(client, &url) {
        let p = p.trim().to_string();
        if !p.is_empty() {
            return p;
        }
    }
    // IMDSv1 fallback
    if !disable_v1 && let Some(p) = imds_v1_get(client, &url) {
        let p = p.trim().to_string();
        if !p.is_empty() {
            return p;
        }
    }
    "aws".to_string()
}

/// Parse host identifier ARN from IMDS identity document.
///
/// Format: `arn:{partition}:ec2:{region}:{accountId}:instance/{instanceId}`
fn parse_host_identifier_from_identity_doc(body: &str, partition: &str) -> Option<String> {
    let doc: serde_json::Value = serde_json::from_str(body).ok()?;
    let region = doc.get("region")?.as_str()?.trim();
    let account_id = doc.get("accountId")?.as_str()?.trim();
    let instance_id = doc.get("instanceId")?.as_str()?.trim();
    Some(format!("arn:{partition}:ec2:{region}:{account_id}:instance/{instance_id}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_program_name() {
        let config = AgentConfig::default();
        assert_eq!(config.program_name, "codedeploy-agent");
    }

    #[test]
    fn default_wait_between_runs() {
        let config = AgentConfig::default();
        assert_eq!(config.wait_between_runs, 30);
    }

    #[test]
    fn default_http_read_timeout() {
        let config = AgentConfig::default();
        assert_eq!(config.http_read_timeout, 80);
    }

    #[test]
    fn default_kill_agent_max_wait() {
        let config = AgentConfig::default();
        assert_eq!(config.kill_agent_max_wait_time_seconds, 7200);
    }

    #[test]
    fn default_max_revisions() {
        let config = AgentConfig::default();
        assert_eq!(config.max_revisions, 5);
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn default_on_premises_path() {
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
    fn default_disable_core_dumps_is_true() {
        assert!(AgentConfig::default().disable_core_dumps);
    }

    #[test]
    fn default_enable_command_port_is_false() {
        assert!(!AgentConfig::default().enable_command_port);
    }

    #[test]
    fn default_reject_symlinks_in_bundle_is_false() {
        assert!(!AgentConfig::default().hardening.reject_symlinks_in_bundle);
    }

    #[test]
    fn default_ignore_ownership_in_bundle_is_false() {
        // Backwards-compatible default: root tar applies the archive's stored
        // uid/gid (`--same-owner` root default); deployments may rely on the
        // resulting non-root write access inside the deployment archive.
        assert!(!AgentConfig::default().hardening.ignore_ownership_in_bundle);
    }

    #[test]
    fn ignore_ownership_in_bundle_parses_from_yaml() {
        let yaml = "ignore_ownership_in_bundle: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.ignore_ownership_in_bundle);

        // Legacy symbol form (`:key:`) must also parse.
        let yaml = ":ignore_ownership_in_bundle: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.ignore_ownership_in_bundle);
    }

    /// Regression for the `HardeningConfig` nesting: hardening flags are
    /// `#[serde(flatten)]`'d into the top-level YAML namespace ALONGSIDE the
    /// `unknown_keys` catch-all (also flattened). Assert a top-level hardening
    /// key deserializes into `hardening` AND is NOT swallowed by the
    /// `unknown_keys` map — the failure mode a flatten-next-to-flatten layout
    /// risks. Also confirms a genuinely unknown key still lands in
    /// `unknown_keys`, so the catch-all is not shadowed by the hardening struct.
    #[test]
    fn hardening_keys_flatten_to_top_level_not_unknown_keys() {
        let yaml = "restrict_agent_dir_permissions: true\nsome_removed_legacy_key: 3\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();

        assert!(
            config.hardening.restrict_agent_dir_permissions,
            "top-level hardening key must deserialize into the nested struct"
        );
        assert!(
            !config.unknown_keys.contains_key("restrict_agent_dir_permissions"),
            "hardening key must be consumed by HardeningConfig, not the unknown-keys catch-all"
        );
        assert!(
            config.unknown_keys.contains_key("some_removed_legacy_key"),
            "genuinely unknown keys must still land in unknown_keys"
        );
    }

    #[test]
    fn default_reject_unconfined_selinux_in_bundle_is_false() {
        assert!(!AgentConfig::default().hardening.reject_unconfined_selinux_in_bundle);
    }

    #[test]
    fn default_reject_unsafe_permissions_in_bundle_is_false() {
        assert!(!AgentConfig::default().hardening.reject_unsafe_permissions_in_bundle);
    }

    #[test]
    fn default_reject_path_traversal_in_bundle_is_false() {
        assert!(!AgentConfig::default().hardening.reject_path_traversal_in_bundle);
    }

    #[test]
    fn default_reject_symlink_permission_targets_is_false() {
        // Backwards-compatible default: permission sinks
        // (chown/chmod/setfacl/semanage) follow a symlinked destination.
        assert!(!AgentConfig::default().hardening.reject_symlink_permission_targets);
    }

    #[test]
    fn reject_symlink_permission_targets_parses_from_yaml() {
        let yaml = "reject_symlink_permission_targets: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.reject_symlink_permission_targets);

        // Legacy symbol form (`:key:`) must also parse.
        let yaml = ":reject_symlink_permission_targets: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.reject_symlink_permission_targets);
    }

    #[test]
    fn default_restrict_agent_dir_permissions_is_false() {
        // Backwards-compatible default: deployment-root dirs are
        // world-readable 0755; host tooling outside the agent reads them.
        assert!(!AgentConfig::default().hardening.restrict_agent_dir_permissions);
    }

    #[test]
    fn restrict_agent_dir_permissions_parses_from_yaml() {
        let yaml = "restrict_agent_dir_permissions: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.restrict_agent_dir_permissions);

        // Legacy symbol form (`:key:`) must also parse.
        let yaml = ":restrict_agent_dir_permissions: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.restrict_agent_dir_permissions);
    }

    #[test]
    fn default_disable_powershell_profile_in_hooks_is_false() {
        // Backwards-compatible default: PowerShell hooks load the user's
        // profile and can prompt.
        assert!(!AgentConfig::default().hardening.disable_powershell_profile_in_hooks);
    }

    #[test]
    fn disable_powershell_profile_in_hooks_parses_from_yaml() {
        let yaml = "disable_powershell_profile_in_hooks: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.disable_powershell_profile_in_hooks);

        let yaml = ":disable_powershell_profile_in_hooks: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.disable_powershell_profile_in_hooks);
    }

    #[test]
    fn default_strip_loader_env_in_hooks_is_false() {
        // Backwards-compatible default: hooks inherit the agent's full env
        // including LD_*.
        assert!(!AgentConfig::default().hardening.strip_loader_env_in_hooks);
    }

    #[test]
    fn default_restrict_hook_env_to_allowlist_is_false() {
        // Backwards-compatible default: hooks inherit the agent's full env,
        // not a minimal allowlist.
        assert!(!AgentConfig::default().hardening.restrict_hook_env_to_allowlist);
    }

    #[test]
    fn hook_env_hardening_flags_parse_from_yaml() {
        let yaml = "strip_loader_env_in_hooks: true\nrestrict_hook_env_to_allowlist: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.strip_loader_env_in_hooks);
        assert!(config.hardening.restrict_hook_env_to_allowlist);
    }

    #[test]
    fn hook_env_hardening_flags_parse_symbol_form() {
        // Legacy symbol form (`:key:`) must also parse, for backwards
        // compatibility with older config files.
        let yaml = ":strip_loader_env_in_hooks: true\n:restrict_hook_env_to_allowlist: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.hardening.strip_loader_env_in_hooks);
        assert!(config.hardening.restrict_hook_env_to_allowlist);
    }

    #[test]
    fn default_archive_max_extraction_size_is_none() {
        assert!(AgentConfig::default().archive_max_extraction_size.is_none());
    }

    #[test]
    fn archive_max_extraction_size_accepts_integer() {
        let yaml = "archive_max_extraction_size: 4294967296\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.archive_max_extraction_size, Some(4_294_967_296));
    }

    #[test]
    fn archive_max_extraction_size_accepts_gb_string() {
        let yaml = "archive_max_extraction_size: \"4GB\"\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.archive_max_extraction_size, Some(4_000_000_000));
    }

    #[test]
    fn archive_max_extraction_size_accepts_gib_string() {
        let yaml = "archive_max_extraction_size: \"4GiB\"\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.archive_max_extraction_size, Some(4 * 1024 * 1024 * 1024));
    }

    #[test]
    fn archive_max_extraction_size_accepts_mb_string() {
        let yaml = "archive_max_extraction_size: \"500MB\"\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.archive_max_extraction_size, Some(500_000_000));
    }

    #[test]
    fn archive_max_extraction_size_none_when_absent() {
        let yaml = "verbose: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert!(config.archive_max_extraction_size.is_none());
    }

    #[test]
    fn parse_byte_size_handles_various_formats() {
        assert_eq!(parse_byte_size("1024").unwrap(), 1024);
        assert_eq!(parse_byte_size("1KB").unwrap(), 1_000);
        assert_eq!(parse_byte_size("1KiB").unwrap(), 1_024);
        assert_eq!(parse_byte_size("1MB").unwrap(), 1_000_000);
        assert_eq!(parse_byte_size("1MiB").unwrap(), 1_048_576);
        assert_eq!(parse_byte_size("1GB").unwrap(), 1_000_000_000);
        assert_eq!(parse_byte_size("1GiB").unwrap(), 1_073_741_824);
        assert_eq!(parse_byte_size("1TB").unwrap(), 1_000_000_000_000);
        assert_eq!(parse_byte_size("  2GB  ").unwrap(), 2_000_000_000);
    }

    #[test]
    fn parse_byte_size_rejects_invalid() {
        assert!(parse_byte_size("abc").is_err());
        assert!(parse_byte_size("1XB").is_err());
    }

    #[test]
    fn parse_byte_size_rejects_fractional() {
        assert!(parse_byte_size("1.5GB").is_err(), "fractional values not supported");
    }

    #[test]
    fn parse_byte_size_rejects_overflow() {
        assert!(parse_byte_size("99999999TB").is_err());
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
    fn shipped_config_parses_and_uses_symbol_keys() {
        // The shipped config uses `:key:` form for backwards compatibility.
        // Lock in both: `:key:` format, and that the agent parses it.
        let shipped = include_str!("../../conf/codedeployagent.yml");
        assert!(shipped.contains(":root_dir:"), "shipped config must use `:key:` form");
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("codedeployagent.yml");
        std::fs::write(&path, shipped).unwrap();
        let config = AgentConfig::from_file(&path).unwrap();
        assert_eq!(config.root_dir, PathBuf::from("/opt/codedeploy-agent/deployment-root"));
        assert_eq!(config.log_dir, PathBuf::from("/var/log/aws/codedeploy-agent"));
        assert_eq!(config.program_name, "codedeploy-agent");
        assert_eq!(config.max_revisions, 5);
        assert!(!config.verbose);
        // `:key:` form, with this agent's default polling interval.
        assert_eq!(config.wait_between_runs, 30);
    }

    #[test]
    fn load_returns_defaults_when_file_missing() {
        // default_config_path() won't exist in test environment
        let config = AgentConfig::load(None).unwrap();
        assert_eq!(config.wait_between_runs, 30);
    }

    #[test]
    fn validate_fips_allows_us_regions() {
        let config = AgentConfig { use_fips_mode: true, ..AgentConfig::default() };
        assert!(config.validate_fips("us-east-1").is_ok());
        assert!(config.validate_fips("us-west-2").is_ok());
        assert!(config.validate_fips("us-gov-west-1").is_ok());
        assert!(config.validate_fips("us-gov-east-1").is_ok());
    }

    #[test]
    fn validate_fips_rejects_non_us_regions() {
        let config = AgentConfig { use_fips_mode: true, ..AgentConfig::default() };
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
disable_core_dumps: false
log_aws_wire: true
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
        assert!(!config.disable_core_dumps);
        assert!(config.log_aws_wire);
    }

    #[test]
    fn log_aws_wire_defaults_false() {
        assert!(!AgentConfig::default().log_aws_wire);
    }

    #[test]
    fn log_aws_wire_parses_symbol_key() {
        // Legacy configs use a leading colon; strip_symbol_keys must map it.
        let config =
            AgentConfig::from_yaml(":log_aws_wire: true\n", Path::new("test.yml")).unwrap();
        assert!(config.log_aws_wire);
    }

    #[test]
    fn from_yaml_ignores_unknown_fields() {
        let yaml = "unknown_key: some_value\nwait_between_runs: 42\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.wait_between_runs, 42);
    }

    #[test]
    fn deprecated_keys_are_captured_as_unknown_but_still_parse() {
        // Options no longer supported land in `unknown_keys` (so they get
        // warned about) yet must NOT break parsing of the real keys.
        let yaml = ":children: 4\n:wait_between_runs: 7\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.wait_between_runs, 7);
        assert!(config.unknown_keys.contains_key("children"));
    }

    #[test]
    fn shipped_config_has_no_unknown_keys() {
        // The struct is the source of truth for known keys (serde routes unknowns
        // into `unknown_keys`). Our own shipped config must contain zero unknown
        // keys, else the agent would warn about its own defaults.
        let shipped = include_str!("../../conf/codedeployagent.yml");
        let config = AgentConfig::from_yaml(shipped, Path::new("codedeployagent.yml")).unwrap();
        assert!(
            config.unknown_keys.is_empty(),
            "shipped config has unrecognised keys: {:?}",
            config.unknown_keys.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn shipped_windows_config_parses_with_no_unknown_keys() {
        // The Windows MSI ships conf.yml (legacy symbol-style keys, absolute
        // Windows paths). It must parse, use the default polling interval, and
        // report no unknown keys.
        let shipped = include_str!("../../conf/conf.yml");
        assert!(shipped.contains(":root_dir:"), "windows config must use `:key:` form");
        let config = AgentConfig::from_yaml(shipped, Path::new("conf.yml")).unwrap();
        assert_eq!(config.wait_between_runs, 30);
        assert!(
            config.unknown_keys.is_empty(),
            "windows config has unrecognised keys: {:?}",
            config.unknown_keys.keys().collect::<Vec<_>>()
        );
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
        if let Ok(region) = std::env::var("AWS_REGION")
            && !region.is_empty()
        {
            assert_eq!(resolve_region(false).unwrap(), region);
        }
    }

    #[test]
    fn resolve_region_error_is_descriptive() {
        let err = ConfigError::RegionNotFound;
        assert!(err.to_string().contains("could not determine AWS region"));
    }

    #[test]
    fn parse_host_identifier_from_identity_doc_constructs_arn() {
        let doc = r#"{"region": "us-west-2", "accountId": "123456789012", "instanceId": "i-0abcdef1234567890"}"#;
        let result = parse_host_identifier_from_identity_doc(doc, "aws");
        assert_eq!(
            result.as_deref(),
            Some("arn:aws:ec2:us-west-2:123456789012:instance/i-0abcdef1234567890")
        );
    }

    #[test]
    fn parse_host_identifier_from_identity_doc_with_gov_partition() {
        let doc =
            r#"{"region": "us-gov-west-1", "accountId": "123456789012", "instanceId": "i-abc123"}"#;
        let result = parse_host_identifier_from_identity_doc(doc, "aws-us-gov");
        assert_eq!(
            result.as_deref(),
            Some("arn:aws-us-gov:ec2:us-gov-west-1:123456789012:instance/i-abc123")
        );
    }

    #[test]
    fn parse_host_identifier_from_identity_doc_trims_whitespace() {
        let doc = r#"{"region": " us-east-1 ", "accountId": " 111 ", "instanceId": " i-x "}"#;
        let result = parse_host_identifier_from_identity_doc(doc, "aws");
        assert_eq!(result.as_deref(), Some("arn:aws:ec2:us-east-1:111:instance/i-x"));
    }

    #[test]
    fn parse_host_identifier_from_identity_doc_returns_none_for_missing_account_id() {
        let doc = r#"{"region": "us-west-2", "instanceId": "i-abc"}"#;
        assert!(parse_host_identifier_from_identity_doc(doc, "aws").is_none());
    }

    #[test]
    fn parse_host_identifier_from_identity_doc_returns_none_for_missing_instance_id() {
        let doc = r#"{"region": "us-west-2", "accountId": "123"}"#;
        assert!(parse_host_identifier_from_identity_doc(doc, "aws").is_none());
    }

    #[test]
    fn parse_host_identifier_from_identity_doc_returns_none_for_missing_region() {
        let doc = r#"{"accountId": "123", "instanceId": "i-abc"}"#;
        assert!(parse_host_identifier_from_identity_doc(doc, "aws").is_none());
    }

    #[test]
    fn parse_host_identifier_from_identity_doc_returns_none_for_invalid_json() {
        assert!(parse_host_identifier_from_identity_doc("{invalid", "aws").is_none());
    }

    #[test]
    fn host_identifier_not_found_error_is_descriptive() {
        let err = ConfigError::HostIdentifierNotFound;
        assert!(err.to_string().contains("could not determine host identifier"));
    }

    #[test]
    fn parse_host_identifier_from_identity_doc_uses_partition_verbatim() {
        let doc = r#"{"region": "us-east-1", "accountId": "123", "instanceId": "i-abc"}"#;
        let result = parse_host_identifier_from_identity_doc(doc, "");
        assert_eq!(result.as_deref(), Some("arn::ec2:us-east-1:123:instance/i-abc"));
    }

    #[test]
    #[ignore = "requires no IMDS and no AWS_REGION env var; cannot be satisfied on EC2 dev desktops"]
    fn resolve_region_and_host_identifier_fails_without_env_or_imds() {
        // Without AWS_REGION/AWS_HOST_IDENTIFIER set and no IMDS available,
        // the function should return an error.
        if std::env::var("AWS_REGION").ok().as_ref().is_some_and(|s| !s.is_empty()) {
            return; // Skip when env var is set (e.g., CI with real credentials).
        }
        let result = resolve_region_and_host_identifier(false);
        assert!(result.is_err(), "expected error without env vars or IMDS");
    }

    #[test]
    fn ensure_dirs_creates_missing_directories() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = AgentConfig {
            pid_dir: dir.path().join("pid"),
            log_dir: dir.path().join("logs"),
            root_dir: dir.path().join("root"),
            ongoing_deployment_tracking: "ongoing".to_string(),
            ..AgentConfig::default()
        };

        config.ensure_dirs().unwrap();

        assert!(config.pid_dir.is_dir());
        assert!(config.log_dir.is_dir());
        assert!(config.root_dir.join("ongoing").is_dir());
    }

    #[test]
    fn ensure_dirs_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let config = AgentConfig {
            pid_dir: dir.path().join("pid"),
            log_dir: dir.path().join("logs"),
            root_dir: dir.path().join("root"),
            ..AgentConfig::default()
        };

        config.ensure_dirs().unwrap();
        config.ensure_dirs().unwrap(); // second call should not fail
    }

    #[test]
    fn default_on_premises_config_path_returns_expected_path() {
        let path = default_on_premises_config_path();
        assert!(
            path.to_str().unwrap().to_lowercase().contains("codedeploy"),
            "expected on-premises config path to contain 'codedeploy', got: {path:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(umask)]
    fn ensure_dirs_default_creates_world_readable_deployment_dirs() {
        // Third-party host processes read the deployment-root dirs, so by
        // default they must be world-readable 0755 (umask-derived modes). The
        // PID dir stays agent-private.
        use nix::sys::stat::{Mode, umask};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let config = AgentConfig {
            pid_dir: dir.path().join("pid"),
            log_dir: dir.path().join("logs"),
            root_dir: dir.path().join("root"),
            ongoing_deployment_tracking: "ongoing".to_string(),
            ..AgentConfig::default()
        };

        // Restrictive umask proves modes are applied via chmod, not mkdir.
        let prev = umask(Mode::from_bits_truncate(0o077));
        let result = config.ensure_dirs();
        umask(prev);
        result.unwrap();

        let pid_mode = std::fs::metadata(&config.pid_dir).unwrap().permissions().mode() & 0o777;
        let root_mode = std::fs::metadata(&config.root_dir).unwrap().permissions().mode() & 0o777;
        let ongoing_mode =
            std::fs::metadata(config.root_dir.join("ongoing")).unwrap().permissions().mode()
                & 0o777;
        let log_mode = std::fs::metadata(&config.log_dir).unwrap().permissions().mode() & 0o777;

        assert_eq!(
            pid_mode, 0o755,
            "pid_dir mode {pid_mode:#o}, want 0755 (world-readable default)"
        );
        assert_eq!(
            root_mode, 0o755,
            "root_dir mode {root_mode:#o}, want 0755 (world-readable default)"
        );
        assert_eq!(
            ongoing_mode, 0o755,
            "ongoing-deployment mode {ongoing_mode:#o}, want 0755 (world-readable default)"
        );
        assert_eq!(
            log_mode, 0o755,
            "log_dir mode {log_mode:#o}, want 0755 (world-readable agent/updater logs)"
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(umask)]
    fn ensure_dirs_restricted_applies_hardened_modes() {
        use nix::sys::stat::{Mode, umask};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let config = AgentConfig {
            pid_dir: dir.path().join("pid"),
            log_dir: dir.path().join("logs"),
            root_dir: dir.path().join("root"),
            ongoing_deployment_tracking: "ongoing".to_string(),
            hardening: HardeningConfig {
                restrict_agent_dir_permissions: true,
                ..Default::default()
            },
            ..AgentConfig::default()
        };

        let prev = umask(Mode::from_bits_truncate(0o077));
        let result = config.ensure_dirs();
        umask(prev);
        result.unwrap();

        let root_mode = std::fs::metadata(&config.root_dir).unwrap().permissions().mode() & 0o777;
        let ongoing_mode =
            std::fs::metadata(config.root_dir.join("ongoing")).unwrap().permissions().mode()
                & 0o777;

        assert_eq!(root_mode, 0o711, "root_dir mode {root_mode:#o}, want 0711 (runas traversal)");
        assert_eq!(ongoing_mode, 0o700, "ongoing-deployment mode {ongoing_mode:#o}, want 0700");
    }

    #[cfg(unix)]
    #[test]
    fn ensure_dirs_default_loosens_dirs_tightened_by_previous_agent() {
        // Upgrade path from an agent version that created these dirs
        // root-only: `ongoing-deployment` already exists at 0700 and must heal
        // to 0755 on the next start, so host tooling regains read access.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let config = AgentConfig {
            pid_dir: dir.path().join("pid"),
            log_dir: dir.path().join("logs"),
            root_dir: dir.path().join("root"),
            ongoing_deployment_tracking: "ongoing".to_string(),
            ..AgentConfig::default()
        };

        let ongoing = config.root_dir.join("ongoing");
        std::fs::create_dir_all(&ongoing).unwrap();
        std::fs::set_permissions(&config.root_dir, std::fs::Permissions::from_mode(0o711)).unwrap();
        std::fs::set_permissions(&ongoing, std::fs::Permissions::from_mode(0o700)).unwrap();

        config.ensure_dirs().unwrap();

        let root_mode = std::fs::metadata(&config.root_dir).unwrap().permissions().mode() & 0o777;
        let ongoing_mode = std::fs::metadata(&ongoing).unwrap().permissions().mode() & 0o777;
        assert_eq!(root_mode, 0o755, "expected 0711 -> 0755 heal, got {root_mode:#o}");
        assert_eq!(ongoing_mode, 0o755, "expected 0700 -> 0755 heal, got {ongoing_mode:#o}");
    }

    #[cfg(unix)]
    #[test]
    fn ensure_dirs_loosens_preexisting_0750_log_dir() {
        // Upgrade path: an older agent left the log dir at 0750. `ensure_dirs`
        // must loosen it to 0755 (create_dir_secure alone only tightens), so
        // deploy-local/update paths that call ensure_dirs without agent_logger
        // still end up world-readable.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let config = AgentConfig {
            pid_dir: dir.path().join("pid"),
            log_dir: dir.path().join("logs"),
            root_dir: dir.path().join("root"),
            ongoing_deployment_tracking: "ongoing".to_string(),
            ..AgentConfig::default()
        };

        std::fs::create_dir_all(&config.log_dir).unwrap();
        std::fs::set_permissions(&config.log_dir, std::fs::Permissions::from_mode(0o750)).unwrap();

        config.ensure_dirs().unwrap();

        let log_mode = std::fs::metadata(&config.log_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(log_mode, 0o755, "expected upgrade to loosen 0750 -> 0755, got {log_mode:#o}");
    }

    /// A fresh host with a customer-configured `root_dir` whose parent does not
    /// exist (e.g. `/export/deployment-root` with no `/export`), with the agent
    /// starting under a restrictive umask (0o027). Every component on the path a
    /// non-root `runas:` hook resolves through must be other-traversable,
    /// including the parent the agent creates — otherwise the first non-root
    /// exec fails with EACCES.
    #[cfg(unix)]
    #[test]
    #[serial_test::serial(umask)]
    fn ensure_dirs_fresh_root_dir_parent_is_other_traversable_under_restrictive_umask() {
        use nix::sys::stat::{Mode, umask};
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let config = AgentConfig {
            pid_dir: dir.path().join("pid"),
            log_dir: dir.path().join("logs"),
            // `export` does not exist: ensure_dirs must create it as a parent.
            root_dir: dir.path().join("export").join("deployment-root"),
            ongoing_deployment_tracking: "ongoing".to_string(),
            ..AgentConfig::default()
        };

        let prev = umask(Mode::from_bits_truncate(0o027));
        let result = config.ensure_dirs();
        umask(prev);
        result.unwrap();

        for component in [&dir.path().join("export"), &config.root_dir] {
            let mode = std::fs::metadata(component).unwrap().permissions().mode() & 0o777;
            assert_ne!(
                mode & 0o001,
                0,
                "{} must be other-traversable (o+x), got {mode:#o}",
                component.display()
            );
        }
    }

    #[test]
    fn strip_symbol_keys_removes_leading_colons() {
        let input = ":wait_between_runs: 5\n:verbose: true\n";
        let expected = "wait_between_runs: 5\nverbose: true";
        assert_eq!(strip_symbol_keys(input), expected);
    }

    #[test]
    fn strip_symbol_keys_preserves_plain_keys() {
        let input = "wait_between_runs: 5\nverbose: true\n";
        let expected = "wait_between_runs: 5\nverbose: true";
        assert_eq!(strip_symbol_keys(input), expected);
    }

    #[test]
    fn strip_symbol_keys_preserves_indentation() {
        let input = "  :nested_key: value\n";
        let expected = "  nested_key: value";
        assert_eq!(strip_symbol_keys(input), expected);
    }

    #[test]
    fn strip_symbol_keys_does_not_strip_colons_in_values() {
        let input = "endpoint: https://example.com\n";
        assert_eq!(strip_symbol_keys(input), "endpoint: https://example.com");
    }

    #[test]
    fn strip_symbol_keys_handles_yaml_document_marker() {
        let input = "---\n:verbose: true\n";
        let expected = "---\nverbose: true";
        assert_eq!(strip_symbol_keys(input), expected);
    }

    #[test]
    fn from_yaml_parses_symbol_style_config() {
        let yaml = ":wait_between_runs: 10\n:verbose: true\n:enable_auth_policy: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.wait_between_runs, 10);
        assert!(config.verbose);
        assert!(config.enable_auth_policy);
    }

    #[test]
    fn from_yaml_parses_mixed_symbol_and_plain_keys() {
        let yaml = ":wait_between_runs: 5\nverbose: true\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.wait_between_runs, 5);
        assert!(config.verbose);
    }

    #[test]
    fn from_yaml_parses_symbol_style_with_quoted_values() {
        let yaml = ":log_dir: '/var/log/aws/codedeploy-agent/'\n:root_dir: '/opt/codedeploy-agent/deployment-root'\n";
        let config = AgentConfig::from_yaml(yaml, Path::new("test.yml")).unwrap();
        assert_eq!(config.log_dir, PathBuf::from("/var/log/aws/codedeploy-agent/"));
        assert_eq!(config.root_dir, PathBuf::from("/opt/codedeploy-agent/deployment-root"));
    }
}
