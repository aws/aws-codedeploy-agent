//! CodeDeploy agent binary entry point.
//!
//! Subcommands:
//! - `start`   — daemonize and begin polling
//! - `stop`    — graceful shutdown
//! - `restart` — stop then start
//! - `status`  — report running/stopped
//! - `_worker` — internal: child worker process. On Windows, auto-detects
//!   Service Control Manager context and dispatches to service_main
//!   when launched by SCM; otherwise runs in console (foreground) mode.

use std::path::{Path, PathBuf};
use std::process;

use clap::{Parser, Subcommand};
use codedeploy_agent::aws_clients::{Credentials, S3Client, S3ClientConfig};
use codedeploy_agent::config::{AgentConfig, resolve_region};
#[cfg(windows)]
use codedeploy_agent::daemon::master::{Master, MasterConfig};
#[cfg(unix)]
use codedeploy_agent::daemon::master::{Master, MasterConfig, StartOutcome, StopOutcome};
use codedeploy_agent::daemon::signal::{ShutdownFlag, register_shutdown_handlers};
use codedeploy_agent::daemon::worker;
use codedeploy_agent::logging::{LogConfig, init_logging};
#[cfg(unix)]
use codedeploy_agent::runtime::FileBasedDeploymentTracker;
#[cfg(unix)]
use codedeploy_agent::system::SystemFileOperations;

/// Environment variable used to forward `--config-file` to the worker subprocess.
///
/// Set by the master process before spawning workers. The worker subprocess
/// reads this on startup to load the same config file the master used.
/// Since the worker is a child process (re-exec with `worker` subcommand), the
/// path is forwarded via env var so the worker can load the same config file.
///
/// This variable is NOT intended to be set externally by users — use
/// `--config-file` instead.
const CONFIG_FILE_ENV: &str = "CODEDEPLOY_AGENT_CONFIG_FILE";

#[derive(Parser)]
#[command(
    name = "codedeploy-agent",
    version,
    disable_version_flag = true,
    about = "AWS CodeDeploy Agent — installs and manages deployments on this instance.",
    long_about = "AWS CodeDeploy Agent\n\n\
        The CodeDeploy agent is a background process that polls the AWS CodeDeploy\n\
        service for deployment commands. It downloads application revisions, runs\n\
        lifecycle hook scripts, and reports deployment status back to the service.\n\n\
        Configuration: /etc/codedeploy-agent/conf/codedeployagent.yml\n\
        Logs:          /var/log/aws/codedeploy-agent/\n\
        State:         /opt/codedeploy-agent/deployment-root/"
)]
struct Cli {
    /// Print version
    #[arg(short = 'v', long = "version", action = clap::ArgAction::Version)]
    version: (),

    /// Path to agent configuration YAML file
    #[arg(long = "config-file", global = true)]
    config_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// Start the agent daemon
    Start,
    /// Stop the agent daemon gracefully
    Stop,
    /// Restart the agent daemon
    Restart,
    /// Check if the agent is running
    Status,
    /// Update the agent to the latest version
    Update,
    /// Run a local deployment without connecting to AWS CodeDeploy
    DeployLocal {
        /// Deployment bundle location. One of:
        ///   - a local filesystem path (default),
        ///   - an s3://bucket/key URI,
        ///   - a public GitHub repo URL: https://github.com/<org>/<repo>
        ///     (HEAD), .../tree/<branch-or-tag>, or .../commit/<sha>,
        ///   - a GitHub API archive URL:
        ///     https://api.github.com/repos/<org>/<repo>/{zipball,tarball}/<ref>.
        ///
        /// Private GitHub repos require --github-token.
        #[arg(
            short = 'l',
            long = "bundle-location",
            default_value = ".",
            verbatim_doc_comment
        )]
        location: String,

        /// Bundle format: directory, tgz, tar, zip
        #[arg(
            short = 't',
            long = "type",
            value_name = "TYPE",
            default_value = "directory"
        )]
        bundle_type: String,

        /// Behavior when target files exist: DISALLOW, OVERWRITE, RETAIN
        #[arg(short = 'b', long = "file-exists-behavior", default_value = "DISALLOW")]
        file_exists_behavior: String,

        /// Deployment group id (subfolder under deployment-root)
        #[arg(
            short = 'g',
            long = "deployment-group",
            default_value = "default-local-deployment-group"
        )]
        deployment_group: String,

        /// Deployment group name exposed as $DEPLOYMENT_GROUP_NAME in hooks
        #[arg(
            short = 'd',
            long = "deployment-group-name",
            default_value = "LocalFleet"
        )]
        deployment_group_name: String,

        /// Application name exposed as $APPLICATION_NAME in hooks
        #[arg(short = 'a', long = "application-name")]
        application_name: Option<String>,

        /// Lifecycle events to run (comma-separated, order matters)
        #[arg(short = 'e', long = "events", value_delimiter = ',')]
        events: Option<Vec<String>>,

        /// Path to agent configuration YAML for this deployment
        #[arg(short = 'c', long = "agent-configuration-file")]
        agent_configuration_file: Option<PathBuf>,

        /// AppSpec filename within the bundle
        #[arg(short = 'A', long = "appspec-filename", default_value = "appspec.yml")]
        appspec_filename: String,

        /// GitHub OAuth token for private-repo bundles (only used for GitHub
        /// `--bundle-location` URLs). When omitted, falls back to the
        /// `CODEDEPLOY_GITHUB_TOKEN` environment variable; if neither is set the
        /// download is anonymous (public repos only). Ignored for non-GitHub
        /// locations. Prefer the environment variable to keep the token out of
        /// the process list and shell history.
        #[arg(long = "github-token", value_name = "TOKEN")]
        github_token: Option<String>,
    },
    /// Internal: worker child process (not user-facing)
    #[command(hide = true)]
    #[allow(non_camel_case_types)]
    _worker,
    /// Install the agent as a Windows service
    #[cfg(windows)]
    InstallService,
    /// Uninstall the agent Windows service
    #[cfg(windows)]
    UninstallService,
    /// Internal: run as a Windows service (invoked by SCM)
    #[cfg(windows)]
    #[command(hide = true)]
    RunAsService,
}

/// A parsed `deploy-local` `--location` value.
///
/// `s3://bucket/key` selects an S3 bundle; a `https://github.com/…` or
/// `https://api.github.com/repos/…` URL selects a GitHub bundle; anything else
/// is treated as a local filesystem path (the pre-existing behavior).
#[derive(Debug, PartialEq, Eq)]
enum BundleLocation {
    Local,
    S3 {
        bucket: String,
        key: String,
    },
    GitHub {
        account: String,
        repository: String,
        commit_id: String,
    },
}

/// Git ref used when a browser-style GitHub URL omits one. GitHub's
/// `zipball`/`tarball` endpoints resolve `HEAD` to the repository's default
/// branch.
const GITHUB_DEFAULT_REF: &str = "HEAD";

/// Parse a `deploy-local` `--location` value.
///
/// Returns:
/// - [`BundleLocation::S3`] for `s3://bucket/key`,
/// - [`BundleLocation::GitHub`] for a `https://github.com/<account>/<repository>`
///   browser URL or `https://api.github.com/repos/…/{zipball,tarball}/<ref>` API URL,
/// - [`BundleLocation::Local`] for any other value.
///
/// Returns `Err` when an `s3://` or GitHub URL is malformed.
fn parse_bundle_location(location: &str) -> Result<BundleLocation, String> {
    if let Some(rest) = location.strip_prefix("s3://") {
        let (bucket, key) = rest
            .split_once('/')
            .ok_or_else(|| format!("invalid S3 location '{location}': expected s3://bucket/key"))?;
        if bucket.is_empty() || key.is_empty() {
            return Err(format!("invalid S3 location '{location}': expected s3://bucket/key"));
        }
        return Ok(BundleLocation::S3 { bucket: bucket.to_string(), key: key.to_string() });
    }

    if let Some(rest) = location.strip_prefix("https://api.github.com/repos/") {
        return parse_github_api_url(location, rest);
    }

    if let Some(rest) = location.strip_prefix("https://github.com/") {
        return parse_github_browser_url(location, rest);
    }

    Ok(BundleLocation::Local)
}

/// Parse the path after `https://api.github.com/repos/`:
/// `<account>/<repository>/{zipball,tarball}/<ref>`. The ref is everything after
/// the `zipball`/`tarball` segment, so slash-containing refs (e.g. `feature/x`)
/// are preserved.
fn parse_github_api_url(location: &str, rest: &str) -> Result<BundleLocation, String> {
    let err = || {
        format!(
            "invalid GitHub location '{location}': expected \
             https://api.github.com/repos/<account>/<repository>/zipball/<ref>"
        )
    };
    // Drop empty segments (tolerates a trailing slash); every legitimate
    // segment and ref component is non-empty.
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    // account / repository / (zipball|tarball) / ref...
    if segments.len() < 4 || !matches!(segments[2], "zipball" | "tarball") {
        return Err(err());
    }
    let account = segments[0];
    let repository = segments[1].trim_end_matches(".git");
    let commit_id = segments[3..].join("/");
    if account.is_empty() || repository.is_empty() || commit_id.is_empty() {
        return Err(err());
    }
    Ok(BundleLocation::GitHub {
        account: account.to_string(),
        repository: repository.to_string(),
        commit_id: commit_id.to_string(),
    })
}

/// Parse the path after `https://github.com/`:
/// `<account>/<repository>[/tree/<ref>|/commit/<ref>][/]`. When no ref segment
/// is present the ref defaults to [`GITHUB_DEFAULT_REF`].
fn parse_github_browser_url(location: &str, rest: &str) -> Result<BundleLocation, String> {
    let err = || {
        format!(
            "invalid GitHub location '{location}': expected \
             https://github.com/<account>/<repository>"
        )
    };
    let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
    if segments.len() < 2 {
        return Err(err());
    }
    let account = segments[0];
    let repository = segments[1].trim_end_matches(".git");
    if account.is_empty() || repository.is_empty() {
        return Err(err());
    }
    // Optional `/tree/<ref>` or `/commit/<ref>` suffix; otherwise default ref.
    let commit_id = if segments.len() >= 4 && matches!(segments[2], "tree" | "commit") {
        segments[3..].join("/")
    } else {
        GITHUB_DEFAULT_REF.to_string()
    };
    Ok(BundleLocation::GitHub {
        account: account.to_string(),
        repository: repository.to_string(),
        commit_id,
    })
}

/// Choose the region to use for an S3 bundle download.
///
/// Prefers the on-premises config region when set; otherwise falls back to the
/// instance/env-resolved region via `resolve`. Mirrors the worker's resolution
/// order (`daemon/worker.rs`): on-premises config region first, IMDS/env next.
fn choose_s3_region<E>(
    onprem_region: &str,
    resolve: impl FnOnce() -> Result<String, E>,
) -> Result<String, E> {
    if onprem_region.is_empty() {
        resolve()
    } else {
        Ok(onprem_region.to_string())
    }
}

/// Build the deployment tracker path from config.
///
/// Joins `root_dir` and `ongoing_deployment_tracking` to form the
/// deployment tracking directory path.
#[cfg(unix)]
fn tracker_path(config: &AgentConfig) -> PathBuf {
    PathBuf::from(&config.root_dir).join(&config.ongoing_deployment_tracking)
}

/// Create a deployment tracker from config.
#[cfg(unix)]
fn make_tracker(config: &AgentConfig) -> FileBasedDeploymentTracker<SystemFileOperations> {
    FileBasedDeploymentTracker::<SystemFileOperations>::new_with_ops(
        tracker_path(config),
        SystemFileOperations::with_policy(config.hardening.restrict_agent_dir_permissions),
    )
}

fn make_master_config(config: &AgentConfig) -> MasterConfig {
    let pid_dir = config.pid_dir.to_string_lossy().to_string();
    MasterConfig {
        pid_dir: pid_dir.clone(),
        kill_wait_secs: config.kill_agent_max_wait_time_seconds,
        enable_command_port: config.enable_command_port,
        state_dir: pid_dir,
        restrict_agent_dir_permissions: config.hardening.restrict_agent_dir_permissions,
        ..MasterConfig::default()
    }
}

/// Create a Master from config.
fn make_master(config: &AgentConfig) -> Master {
    Master::new(make_master_config(config))
}

fn run(command: &Command, config_file: Option<&Path>) {
    // Load config before anything else — load once, pass to all consumers.
    let config = match AgentConfig::load(config_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {e}");
            process::exit(1);
        },
    };

    if config.disable_core_dumps {
        codedeploy_agent::daemon::core_dumps::disable();
    }

    // Validate FIPS region eagerly so the master refuses to start when
    // `use_fips_mode: true` is set in a non-FIPS region. Skipped when
    // `use_fips_mode: false` to avoid an unnecessary IMDS round-trip.
    if config.use_fips_mode {
        let region = match resolve_region(config.disable_imds_v1) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "Failed to resolve AWS region for FIPS validation: {e}. \
                     Set AWS_REGION env var or run on an EC2 instance with IMDS enabled, \
                     or set use_fips_mode: false in the agent config."
                );
                process::exit(1);
            },
        };
        if let Err(e) = config.validate_fips(&region) {
            eprintln!(
                "use_fips_mode is enabled but the resolved region '{region}' \
                 does not have FIPS endpoints: {e}. Set use_fips_mode: false \
                 in the agent config or deploy the agent in a US region \
                 (us-east-*, us-west-*, us-gov-*)."
            );
            process::exit(1);
        }
    }

    // Tighten the default umask to 0027: agent files/dirs created without an
    // explicit mode land at 0640 / 0750, and adm-group ops users can still tail.
    #[cfg(unix)]
    {
        use nix::sys::stat::{Mode, umask};
        umask(Mode::from_bits_truncate(0o027));
    }

    // Create log and PID directories.
    if let Err(e) = config.ensure_dirs() {
        eprintln!("Failed to create required directories: {e}");
        process::exit(1);
    }

    // Forward config file path to worker subprocess via env var.
    // Only needed for commands that spawn worker subprocesses.
    if let Some(p) = config_file
        && matches!(command, Command::Start | Command::Restart)
    {
        // SAFETY: set_var is called in the master process before any threads
        // are spawned. The child worker reads this env var on startup.
        unsafe { std::env::set_var(CONFIG_FILE_ENV, p) };
    }

    // Write the `.version` file, which the installer package does not ship.
    // The Unix master writes it here; the Windows service writes it in
    // `service_work_loop`. Best-effort — a failure must not stop the agent.
    #[cfg(unix)]
    if matches!(command, Command::Start | Command::Restart)
        && let Err(e) = codedeploy_agent::system::version_file::write()
    {
        eprintln!("Warning: failed to write agent .version file: {e}");
    }

    match command {
        #[cfg(unix)]
        Command::Start => {
            let master = make_master(&config);
            match master.start() {
                Ok(StartOutcome::Started) => {},
                Ok(StartOutcome::AlreadyRunning { pid }) => {
                    eprintln!("Agent is already running (pid {pid})");
                    process::exit(1);
                },
                Err(e) => {
                    eprintln!("Failed to start agent: {e}");
                    process::exit(1);
                },
            }
        },
        #[cfg(windows)]
        Command::Start => {
            eprintln!(
                "Use `sc start codedeployagent` on Windows, or run `codedeploy-agent install-service` first"
            );
            process::exit(1);
        },
        #[cfg(unix)]
        Command::Stop => {
            let master = make_master(&config);
            let tracker = make_tracker(&config);
            if let Err(e) = master.stop(Some(&tracker)) {
                eprintln!("Failed to stop agent: {e}");
                process::exit(1);
            }
        },
        #[cfg(windows)]
        Command::Stop => {
            eprintln!("Use `sc stop codedeployagent` on Windows");
            process::exit(1);
        },
        #[cfg(unix)]
        Command::Restart => {
            let master = make_master(&config);
            let tracker = make_tracker(&config);
            // Stop — propagate deployment and timeout errors, ignore "not running"
            match master.stop(Some(&tracker)) {
                Ok(StopOutcome::Stopped | StopOutcome::NotRunning) => {},
                Err(e) => {
                    eprintln!("Failed to restart agent: {e}");
                    process::exit(1);
                },
            }
            // Fresh master needed: the previous instance's ShutdownFlag may
            // have been set during stop, and start() registers new signal
            // handlers against the flag.
            // NOTE: signal-hook's flag::register appends handlers globally and does not
            // support unregistration. Each restart leaks one Arc<AtomicBool> (~32 bytes).
            // Acceptable since restarts are rare. Consider signal_hook::iterator if this
            // becomes a concern.
            let fresh = make_master(&config);
            match fresh.start() {
                Ok(StartOutcome::Started) => {},
                Ok(StartOutcome::AlreadyRunning { pid }) => {
                    eprintln!("Agent is already running (pid {pid})");
                    process::exit(1);
                },
                Err(e) => {
                    eprintln!("Failed to start agent: {e}");
                    process::exit(1);
                },
            }
        },
        #[cfg(windows)]
        Command::Restart => {
            eprintln!("Use `sc stop codedeployagent` then `sc start codedeployagent` on Windows");
            process::exit(1);
        },
        #[cfg(unix)]
        Command::Status => {
            let master = make_master(&config);
            match master.status() {
                Ok(true) => {
                    println!("The AWS CodeDeploy agent is running.");
                    process::exit(0);
                },
                Ok(false) => {
                    println!("The AWS CodeDeploy agent is not running.");
                    process::exit(3); // LSB: program is not running
                },
                Err(e) => {
                    eprintln!("Error checking status: {e}");
                    process::exit(4); // LSB: status unknown
                },
            }
        },
        #[cfg(windows)]
        Command::Status => {
            eprintln!("Use `sc query codedeployagent` on Windows");
            process::exit(1);
        },
        #[cfg(unix)]
        Command::Update => run_update(&config),
        #[cfg(windows)]
        Command::Update => {
            eprintln!(
                "`update` is not supported on Windows. Update via the agent MSI installer, \
                 or let the CodeDeploy service deliver UpdateDeploymentAgent."
            );
            process::exit(1);
        },
        Command::DeployLocal {
            location,
            bundle_type,
            file_exists_behavior,
            deployment_group,
            deployment_group_name,
            application_name,
            events,
            agent_configuration_file,
            appspec_filename,
            github_token,
        } => {
            // `-c`/`--agent-configuration-file` selects the config for this
            // command. Reload from it when given, else use the global config
            // already loaded above.
            let local_config = match agent_configuration_file {
                Some(path) => match AgentConfig::load(Some(path.as_path())) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Failed to load config: {e}");
                        process::exit(1);
                    },
                },
                None => config,
            };
            // Flag takes precedence; fall back to the environment variable so
            // callers can keep the token out of the process list / shell
            // history. `None` from both leaves the GitHub download anonymous.
            let github_token = github_token
                .clone()
                .or_else(|| std::env::var("CODEDEPLOY_GITHUB_TOKEN").ok())
                .filter(|t| !t.is_empty());
            run_deploy_local(
                &local_config,
                location,
                bundle_type,
                file_exists_behavior,
                deployment_group,
                deployment_group_name,
                application_name.as_deref(),
                events.as_deref(),
                appspec_filename,
                github_token.as_deref(),
            );
        },
        Command::_worker => {
            // On Windows, auto-detect Service Control Manager context.
            // If launched by SCM (e.g. MSI registered with "worker" arg by
            // mistake), hand off to the full service_main path. If not under
            // SCM (error 1063), fall through to console mode below.
            #[cfg(windows)]
            {
                use codedeploy_agent::daemon::windows_service::{self, DispatchOutcome};
                match windows_service::try_run() {
                    Ok(DispatchOutcome::RanAsService) => return,
                    Ok(DispatchOutcome::NotLaunchedByScm) => {
                        // Not under SCM — fall through to console mode below.
                    },
                    Err(e) => {
                        eprintln!("Service dispatch failed: {e}");
                        process::exit(1);
                    },
                }
            }
            // Initialize logging for the worker subprocess.
            let log_config = LogConfig {
                log_dir: config.log_dir.clone(),
                verbose: config.verbose,
                program_name: config.program_name.clone(),
                root_dir: config.root_dir.clone(),
                restrict_permissions: config.hardening.restrict_agent_dir_permissions,
                restrict_log_permissions: config.hardening.restrict_log_dir_permissions,
            };
            let _guard = match init_logging(&log_config) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("Failed to initialize logging: {e}");
                    process::exit(1);
                },
            };

            let shutdown = ShutdownFlag::new();
            if let Err(e) = register_shutdown_handlers(&shutdown) {
                eprintln!("Failed to register signal handlers: {e}");
                process::exit(1);
            }
            worker::bind_lifetime_to_parent();
            worker::run(&shutdown, &config);
        },
        #[cfg(windows)]
        Command::InstallService => {
            use codedeploy_agent::daemon::windows_service;
            match windows_service::install() {
                Ok(()) => println!("Service 'codedeployagent' installed successfully"),
                Err(e) => {
                    eprintln!("Failed to install service: {e}");
                    process::exit(1);
                },
            }
        },
        #[cfg(windows)]
        Command::UninstallService => {
            use codedeploy_agent::daemon::windows_service;
            match windows_service::uninstall() {
                Ok(()) => println!("Service 'codedeployagent' uninstalled successfully"),
                Err(e) => {
                    eprintln!("Failed to uninstall service: {e}");
                    process::exit(1);
                },
            }
        },
        #[cfg(windows)]
        Command::RunAsService => {
            use codedeploy_agent::daemon::windows_service;
            if let Err(e) = windows_service::run() {
                eprintln!("Service failed: {e}");
                process::exit(1);
            }
        },
    }
}

/// Exit code for validation errors (bad input before execution starts).
const EXIT_VALIDATION: i32 = 2;

/// The canonical lifecycle event order used when `--events` is omitted,
/// and the base set for the hook mapping.
const DEFAULT_ORDERED_LIFECYCLE_EVENTS: &[&str] = &[
    "BeforeBlockTraffic",
    "AfterBlockTraffic",
    "ApplicationStop",
    "DownloadBundle",
    "BeforeInstall",
    "Install",
    "AfterInstall",
    "ApplicationStart",
    "ValidateService",
    "BeforeAllowTraffic",
    "AfterAllowTraffic",
];

/// Lifecycle events that are always injected when omitted.
const REQUIRED_LIFECYCLE_EVENTS: &[&str] = &["DownloadBundle", "Install"];

/// Events that use the new revision (excludes `BeforeInstall`).
/// These may not precede `DownloadBundle` or `Install`.
const EVENTS_USING_NEW_REVISION: &[&str] = &[
    "AfterInstall",
    "ApplicationStart",
    "BeforeAllowTraffic",
    "AfterAllowTraffic",
    "ValidateService",
];

/// The only events allowed before `DownloadBundle`.
const EVENTS_BEFORE_DOWNLOAD_BUNDLE: &[&str] =
    &["BeforeBlockTraffic", "AfterBlockTraffic", "ApplicationStop"];

const VALID_BUNDLE_TYPES: &[&str] = &["tar", "tgz", "zip", "directory"];
const VALID_FILE_EXISTS_BEHAVIORS: &[&str] = &["DISALLOW", "OVERWRITE", "RETAIN"];

#[cfg(unix)]
fn run_update(config: &AgentConfig) {
    use codedeploy_agent::host_command::commands::UpdateAgentCommand;

    // Resolve credentials + region exactly as the worker / S3 deploy path do:
    // on-premises config region first, then AWS_REGION env / IMDS.
    let mut credentials = match Credentials::load(&config.on_premises_config_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to resolve credentials for agent update: {e}");
            process::exit(1);
        },
    };
    let region =
        match choose_s3_region(&credentials.region, || resolve_region(config.disable_imds_v1)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "Failed to resolve AWS region for agent update: {e}. \
                     Set AWS_REGION or configure 'region' in the on-premises config."
                );
                process::exit(1);
            },
        };
    credentials.region = region.clone();

    let s3_config = S3ClientConfig {
        use_fips: config.use_fips_mode,
        proxy_uri: config.proxy_uri.clone(),
        wire_log: config
            .log_aws_wire
            .then(|| (config.log_dir.clone(), config.program_name.clone())),
        ..Default::default()
    };
    let s3_client = match S3Client::new(credentials, &s3_config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to create S3 client for agent update: {e}");
            process::exit(1);
        },
    };

    println!("Updating the AWS CodeDeploy agent...");
    let command = UpdateAgentCommand::new(region, Some(s3_client))
        .with_restrict_log_permissions(config.hardening.restrict_log_dir_permissions);
    match command.execute() {
        Ok(messages) => {
            for msg in messages {
                println!("{msg}");
            }
            println!(
                "Agent update script completed — the agent package's post-install \
                 scripts will restart the agent."
            );
        },
        Err(e) => {
            eprintln!("Agent update failed: {e}");
            process::exit(1);
        },
    }
}

#[allow(clippy::too_many_arguments)]
fn run_deploy_local(
    config: &AgentConfig,
    location: &str,
    bundle_type: &str,
    file_exists_behavior: &str,
    deployment_group: &str,
    deployment_group_name: &str,
    application_name: Option<&str>,
    events: Option<&[String]>,
    appspec_filename: &str,
    github_token: Option<&str>,
) {
    use codedeploy_agent::application_specification::AppSpec;
    use codedeploy_agent::deployment_specification::types::{
        DeploymentSpec, RevisionLocation, RevisionSource,
    };

    // `--application-name` defaults to the bundle location.
    let application_name = application_name.unwrap_or(location).to_string();

    // An `s3://bucket/key` location is downloaded via the same S3 bundle
    // downloader that service deployments use. Local-path handling below
    // is left unchanged.
    match parse_bundle_location(location) {
        Err(msg) => {
            eprintln!("Validation error: {msg}");
            process::exit(EXIT_VALIDATION);
        },
        Ok(BundleLocation::S3 { bucket, key }) => {
            run_deploy_local_s3(
                config,
                &bucket,
                &key,
                location,
                bundle_type,
                file_exists_behavior,
                deployment_group,
                deployment_group_name,
                &application_name,
                events,
                appspec_filename,
            );
            return;
        },
        Ok(BundleLocation::GitHub { account, repository, commit_id }) => {
            run_deploy_local_github(
                config,
                &account,
                &repository,
                &commit_id,
                location,
                bundle_type,
                file_exists_behavior,
                deployment_group,
                deployment_group_name,
                &application_name,
                events,
                appspec_filename,
                github_token,
            );
            return;
        },
        Ok(BundleLocation::Local) => {},
    }

    let location = Path::new(location);
    let location_str = location.display().to_string();
    let is_dir = location.is_dir();

    // --- Validation phase (exit 2 on failure) ---

    if !location.exists() {
        eprintln!("Validation error: bundle not found: {location_str}");
        process::exit(EXIT_VALIDATION);
    }

    if !is_dir && !location.is_file() {
        eprintln!(
            "Validation error: bundle path is not a regular file or directory: {location_str}"
        );
        process::exit(EXIT_VALIDATION);
    }

    if !VALID_FILE_EXISTS_BEHAVIORS.contains(&file_exists_behavior) {
        eprintln!(
            "Validation error: invalid --file-exists-behavior '{file_exists_behavior}'. \
             Must be one of: DISALLOW, OVERWRITE, RETAIN"
        );
        process::exit(EXIT_VALIDATION);
    }

    if !VALID_BUNDLE_TYPES.contains(&bundle_type) {
        eprintln!(
            "Validation error: invalid bundle type '{bundle_type}'. \
             Must be one of: tar, tgz, zip, directory"
        );
        process::exit(EXIT_VALIDATION);
    }

    if is_dir && bundle_type != "directory" {
        eprintln!(
            "Validation error: location is a directory but bundle type is '{bundle_type}'. \
             Use --type directory"
        );
        process::exit(EXIT_VALIDATION);
    }

    if !is_dir && bundle_type == "directory" {
        eprintln!(
            "Validation error: location is a file but bundle type is 'directory'. \
             Use --type tar, tgz, or zip"
        );
        process::exit(EXIT_VALIDATION);
    }

    if let Some(specified) = events
        && let Err(msg) = validate_event_ordering(specified)
    {
        eprintln!("Validation error: {msg}");
        process::exit(EXIT_VALIDATION);
    }

    // For directories, validate appspec upfront before execution
    if is_dir {
        let appspec_path = location.join(appspec_filename);
        if !appspec_path.exists() {
            eprintln!(
                "Validation error: {appspec_filename} not found in bundle directory: {}",
                appspec_path.display()
            );
            process::exit(EXIT_VALIDATION);
        }
        match AppSpec::from_file(&appspec_path) {
            Ok(spec) => {
                validate_appspec_hooks(&spec, appspec_filename, events);
            },
            Err(e) => {
                eprintln!("Validation error: failed to parse {appspec_filename}: {e}");
                process::exit(EXIT_VALIDATION);
            },
        }
    }

    // --- Execution phase (exit 1 on failure) ---

    let revision_source = if is_dir {
        RevisionSource::LocalDirectory
    } else {
        RevisionSource::LocalFile
    };

    let spec = DeploymentSpec {
        deployment_id: format!("local-{}", std::process::id()),
        deployment_group_id: deployment_group.to_string(),
        deployment_group_name: deployment_group_name.to_string(),
        application_name,
        deployment_creator: "local-user".to_string(),
        deployment_type: "IN_PLACE".to_string(),
        app_spec_path: appspec_filename.to_string(),
        file_exists_behavior: file_exists_behavior.to_string(),
        revision_source,
        revision: RevisionLocation::Local {
            location: location_str.clone(),
            bundle_type: bundle_type.to_string(),
        },
        all_possible_lifecycle_events: None,
        reuse_archive_from_deployment_id: None,
    };

    execute_local_deployment(
        config,
        &spec,
        None,
        "local",
        &location_str,
        bundle_type,
        is_dir,
        appspec_filename,
        events,
    );
}

/// Run the download → install → hooks command sequence for a local deployment.
///
/// Shared by the local-path and S3 (`run_deploy_local_s3`) entry points. The
/// caller supplies a fully built [`DeploymentSpec`]; this drives the command
/// sequence and validates `appspec.yml` after the bundle is extracted (for
/// non-directory bundles). Exits the process on failure: `EXIT_VALIDATION` for
/// a bad appspec, `1` for a command failure.
#[allow(clippy::too_many_arguments)]
fn execute_local_deployment(
    config: &AgentConfig,
    spec: &codedeploy_agent::deployment_specification::types::DeploymentSpec,
    s3_client: Option<S3Client>,
    region: &str,
    bundle_display: &str,
    bundle_type: &str,
    is_dir: bool,
    appspec_filename: &str,
    events: Option<&[String]>,
) {
    use codedeploy_agent::application_specification::AppSpec;
    use codedeploy_agent::host_command::{CommandDispatcher, DeploymentArchives};
    use std::sync::Arc;

    let deployment_root = config.root_dir.clone();
    let instructions_dir = config.root_dir.join("deployment-instructions");
    let archives = Arc::new(
        DeploymentArchives::new(deployment_root, instructions_dir, config.max_revisions as usize)
            .with_restrict_permissions(config.hardening.restrict_agent_dir_permissions),
    );

    let hook_mapping = build_local_hook_mapping(events);

    let dispatcher = CommandDispatcher::new(
        archives.clone(),
        s3_client,
        hook_mapping,
        region,
        None,
        Arc::new(config.clone()),
    );

    println!("Starting local deployment...");
    println!("  Bundle: {bundle_display}");
    println!("  Type: {bundle_type}");
    println!("  Deployment ID: {}", spec.deployment_id);

    let commands = build_local_command_sequence(events);

    for cmd_name in &commands {
        println!("  Executing: {cmd_name}");
        match dispatcher.execute_command(cmd_name, spec) {
            Ok(_) => {
                println!("    {cmd_name} succeeded");

                // After DownloadBundle, validate appspec in the extracted archive
                if cmd_name == "DownloadBundle" && !is_dir {
                    let archive_dir =
                        archives.archive_dir(&spec.deployment_group_id, &spec.deployment_id);
                    let appspec_path = archive_dir.join(appspec_filename);
                    match AppSpec::from_file(&appspec_path) {
                        Ok(parsed) => {
                            validate_appspec_hooks(&parsed, appspec_filename, events);
                        },
                        Err(e) => {
                            eprintln!(
                                "Validation error: failed to parse {appspec_filename} \
                                 after extraction: {e}"
                            );
                            process::exit(EXIT_VALIDATION);
                        },
                    }
                }
            },
            Err(e) => {
                eprintln!("    {cmd_name} failed: {e}");
                process::exit(1);
            },
        }
    }

    println!("Local deployment succeeded.");
}

/// Run a local deployment from an `s3://bucket/key` bundle.
///
/// Validates arguments, resolves the S3 region from the agent's on-premises
/// config or instance/env (matching the worker), builds an `S3Client`, and
/// drives the same download → install → hooks flow as a local-file bundle via
/// [`execute_local_deployment`]. Exits with `EXIT_VALIDATION` on bad input
/// and `1` on download/region/client failure.
#[allow(clippy::too_many_arguments)]
fn run_deploy_local_s3(
    config: &AgentConfig,
    bucket: &str,
    key: &str,
    location_str: &str,
    bundle_type: &str,
    file_exists_behavior: &str,
    deployment_group: &str,
    deployment_group_name: &str,
    application_name: &str,
    events: Option<&[String]>,
    appspec_filename: &str,
) {
    use codedeploy_agent::deployment_specification::types::{
        DeploymentSpec, RevisionLocation, RevisionSource,
    };

    // --- Validation phase (exit 2 on failure) ---

    if !VALID_FILE_EXISTS_BEHAVIORS.contains(&file_exists_behavior) {
        eprintln!(
            "Validation error: invalid --file-exists-behavior '{file_exists_behavior}'. \
             Must be one of: DISALLOW, OVERWRITE, RETAIN"
        );
        process::exit(EXIT_VALIDATION);
    }

    // An S3 bundle is always an archive — `--type directory` is invalid here.
    if bundle_type == "directory" || !VALID_BUNDLE_TYPES.contains(&bundle_type) {
        eprintln!(
            "Validation error: invalid bundle type '{bundle_type}' for an S3 bundle. \
             Must be one of: tar, tgz, zip"
        );
        process::exit(EXIT_VALIDATION);
    }

    if let Some(specified) = events
        && let Err(msg) = validate_event_ordering(specified)
    {
        eprintln!("Validation error: {msg}");
        process::exit(EXIT_VALIDATION);
    }

    // --- Execution phase (exit 1 on failure) ---

    // Resolve region + credentials the same way the worker does: on-premises
    // config first, then AWS_REGION env / IMDS.
    let mut credentials = match Credentials::load(&config.on_premises_config_file) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to resolve credentials for S3 download: {e}");
            process::exit(1);
        },
    };
    let region =
        match choose_s3_region(&credentials.region, || resolve_region(config.disable_imds_v1)) {
            Ok(r) => r,
            Err(e) => {
                eprintln!(
                    "Failed to resolve AWS region for S3 bundle download: {e}. \
                 Set AWS_REGION or configure 'region' in the on-premises config."
                );
                process::exit(1);
            },
        };
    credentials.region = region.clone();

    let s3_config = S3ClientConfig {
        use_fips: config.use_fips_mode,
        proxy_uri: config.proxy_uri.clone(),
        wire_log: config
            .log_aws_wire
            .then(|| (config.log_dir.clone(), config.program_name.clone())),
        ..Default::default()
    };
    let s3_client = match S3Client::new(credentials, &s3_config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to create S3 client: {e}");
            process::exit(1);
        },
    };

    let spec = DeploymentSpec {
        deployment_id: format!("local-{}", std::process::id()),
        deployment_group_id: deployment_group.to_string(),
        deployment_group_name: deployment_group_name.to_string(),
        application_name: application_name.to_string(),
        deployment_creator: "local-user".to_string(),
        deployment_type: "IN_PLACE".to_string(),
        app_spec_path: appspec_filename.to_string(),
        file_exists_behavior: file_exists_behavior.to_string(),
        revision_source: RevisionSource::S3,
        revision: RevisionLocation::S3 {
            bucket: bucket.to_string(),
            key: key.to_string(),
            bundle_type: bundle_type.to_string(),
            version: None,
            etag: None,
        },
        all_possible_lifecycle_events: None,
        reuse_archive_from_deployment_id: None,
    };

    execute_local_deployment(
        config,
        &spec,
        Some(s3_client),
        &region,
        location_str,
        bundle_type,
        false,
        appspec_filename,
        events,
    );
}

/// Resolve the bundle type for a GitHub source. Only `tar` or `zip` are valid
/// (the `GitHubDownloader` accepts nothing else), so everything else — `tgz`,
/// the `directory` default, unknowns — coerces to `zip`.
fn github_bundle_type(bundle_type: &str) -> &'static str {
    if bundle_type == "tar" { "tar" } else { "zip" }
}

/// Run a local deployment from a GitHub `https://…` bundle URL.
///
/// Drives the same download → install → hooks flow as an S3 bundle via
/// [`execute_local_deployment`], using the `GitHubDownloader` path. Supports
/// both public and private repos: when `github_token` is `Some`, the download
/// is authenticated (`Authorization: token <t>`); when `None`, it stays
/// anonymous (public repos only). The token comes from the `--github-token`
/// flag or the `CODEDEPLOY_GITHUB_TOKEN` environment variable. No AWS
/// credentials are resolved on this path. Bundle type is coerced via
/// [`github_bundle_type`] (only `zip`/`tar` are valid).
///
/// Exits with `EXIT_VALIDATION` on bad input and `1` on download failure.
#[allow(clippy::too_many_arguments)]
fn run_deploy_local_github(
    config: &AgentConfig,
    account: &str,
    repository: &str,
    commit_id: &str,
    location_str: &str,
    bundle_type: &str,
    file_exists_behavior: &str,
    deployment_group: &str,
    deployment_group_name: &str,
    application_name: &str,
    events: Option<&[String]>,
    appspec_filename: &str,
    github_token: Option<&str>,
) {
    use codedeploy_agent::deployment_specification::types::{
        DeploymentSpec, RevisionLocation, RevisionSource,
    };

    // --- Validation phase (exit 2 on failure) ---

    if !VALID_FILE_EXISTS_BEHAVIORS.contains(&file_exists_behavior) {
        eprintln!(
            "Validation error: invalid --file-exists-behavior '{file_exists_behavior}'. \
             Must be one of: DISALLOW, OVERWRITE, RETAIN"
        );
        process::exit(EXIT_VALIDATION);
    }

    let resolved_bundle_type = github_bundle_type(bundle_type);

    if let Some(specified) = events
        && let Err(msg) = validate_event_ordering(specified)
    {
        eprintln!("Validation error: {msg}");
        process::exit(EXIT_VALIDATION);
    }

    // --- Execution phase (exit 1 on failure) ---

    // A token (from --github-token or CODEDEPLOY_GITHUB_TOKEN) switches the
    // download to authenticated for private repos; without one it stays
    // anonymous (public repos only). No credentials or S3Client are needed —
    // the GitHubDownloader fetches over HTTPS directly.
    let anonymous = github_token.is_none();
    let auth_token = github_token.map(str::to_string);
    let spec = DeploymentSpec {
        deployment_id: format!("local-{}", std::process::id()),
        deployment_group_id: deployment_group.to_string(),
        deployment_group_name: deployment_group_name.to_string(),
        application_name: application_name.to_string(),
        deployment_creator: "local-user".to_string(),
        deployment_type: "IN_PLACE".to_string(),
        app_spec_path: appspec_filename.to_string(),
        file_exists_behavior: file_exists_behavior.to_string(),
        revision_source: RevisionSource::GitHub,
        revision: RevisionLocation::GitHub {
            account: account.to_string(),
            repository: repository.to_string(),
            commit_id: commit_id.to_string(),
            anonymous,
            auth_token,
            bundle_type: Some(resolved_bundle_type.to_string()),
        },
        all_possible_lifecycle_events: None,
        reuse_archive_from_deployment_id: None,
    };

    execute_local_deployment(
        config,
        &spec,
        None,
        "",
        location_str,
        resolved_bundle_type,
        false,
        appspec_filename,
        events,
    );
}

fn validate_appspec_hooks(
    app_spec: &codedeploy_agent::application_specification::AppSpec,
    appspec_filename: &str,
    events: Option<&[String]>,
) {
    if let Some(specified) = events {
        let hook_events: Vec<&str> = app_spec.hooks().events().collect();
        for event in specified {
            // DownloadBundle/Install are internal — never AppSpec hooks.
            if REQUIRED_LIFECYCLE_EVENTS.contains(&event.as_str()) {
                continue;
            }
            let scripts = app_spec.hooks().get(event);
            if scripts.is_empty() && !hook_events.contains(&event.as_str()) {
                eprintln!(
                    "Warning: lifecycle event '{event}' requested but not defined in \
                     {appspec_filename} (will be skipped)"
                );
            }
        }
    }
}

fn build_local_hook_mapping(
    events: Option<&[String]>,
) -> std::collections::HashMap<String, Vec<String>> {
    use std::collections::HashMap;

    // Build hook mapping: (DEFAULT ∪ ordered_lifecycle_events) minus REQUIRED,
    // each event mapped to itself. Includes custom event names.
    let mut all: Vec<String> =
        DEFAULT_ORDERED_LIFECYCLE_EVENTS.iter().map(|s| (*s).to_string()).collect();
    for event in ordered_lifecycle_events(events) {
        if !all.contains(&event) {
            all.push(event);
        }
    }

    let mut mapping = HashMap::new();
    for event in all {
        if !REQUIRED_LIFECYCLE_EVENTS.contains(&event.as_str()) {
            mapping.insert(event.clone(), vec![event]);
        }
    }
    mapping
}

/// Returns the user's events, or the default ordered set when none were given.
fn ordered_lifecycle_events(events: Option<&[String]>) -> Vec<String> {
    match events {
        Some(specified) if !specified.is_empty() => specified.to_vec(),
        _ => DEFAULT_ORDERED_LIFECYCLE_EVENTS.iter().map(|s| (*s).to_string()).collect(),
    }
}

/// Prepend any required events not already present, then fix DownloadBundle/Install ordering.
fn add_download_bundle_and_install_events(events: &[String]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();
    for req in REQUIRED_LIFECYCLE_EVENTS {
        if !events.iter().any(|e| e.as_str() == *req) {
            result.push((*req).to_string());
        }
    }
    result.extend(events.iter().cloned());
    // If Install appears before DownloadBundle, swap them.
    let dl = result.iter().position(|e| e == "DownloadBundle");
    let install = result.iter().position(|e| e == "Install");
    if let (Some(dl), Some(install)) = (dl, install)
        && install < dl
    {
        result.swap(dl, install);
    }
    result
}

/// Final ordered command list for a local deployment.
fn build_local_command_sequence(events: Option<&[String]>) -> Vec<String> {
    add_download_bundle_and_install_events(&ordered_lifecycle_events(events))
}

/// Validates event ordering: certain events may not precede DownloadBundle/Install.
/// Returns an error message on bad order.
fn validate_event_ordering(events: &[String]) -> Result<(), String> {
    let before = |marker: &str| -> Vec<&str> {
        events.iter().map(String::as_str).take_while(|e| *e != marker).collect()
    };

    if events.iter().any(|e| e == "DownloadBundle") {
        let pre = before("DownloadBundle");
        let bad = pre.iter().any(|e| EVENTS_USING_NEW_REVISION.contains(e) || *e == "Install");
        if bad {
            return Err(format!(
                "The only events that can be specified before DownloadBundle are {}. \
                 Please fix the order of your specified events",
                EVENTS_BEFORE_DOWNLOAD_BUNDLE.join(",")
            ));
        }
    }

    if events.iter().any(|e| e == "Install") {
        let pre = before("Install");
        let bad = pre.iter().any(|e| EVENTS_USING_NEW_REVISION.contains(e));
        if bad {
            return Err(format!(
                "The only events that can be specified before Install are {},DownloadBundle,BeforeInstall. \
                 Please fix the order of your specified events",
                EVENTS_BEFORE_DOWNLOAD_BUNDLE.join(",")
            ));
        }
    }

    Ok(())
}

fn main() {
    let cli = Cli::parse_from(legacy_local_argv(std::env::args_os()));
    // Resolve config path: CLI flag takes precedence, then env var (set by master
    // for worker subprocess), then default path.
    let config_path = cli
        .config_file
        .or_else(|| std::env::var(CONFIG_FILE_ENV).ok().map(PathBuf::from));

    run(&cli.command, config_path.as_deref());
}

/// Program basename that selects the legacy `codedeploy-local` compatibility
/// mode. Matches the historical standalone tool's name (see
/// <https://docs.aws.amazon.com/codedeploy/latest/userguide/deployments-local.html>).
const LEGACY_LOCAL_PROGRAM: &str = "codedeploy-local";

/// Rewrite argv for backwards compatibility with the standalone `codedeploy-local`
/// tool (multi-call binary dispatch).
///
/// The local-deployment tool used to ship as a separate `codedeploy-local`
/// executable invoked as `codedeploy-local [options]`. It is now a subcommand of
/// the single agent binary (`codedeploy-agent deploy-local [options]`). To keep
/// the documented invocation working, the packaging installs a `codedeploy-local`
/// symlink (or copy) alongside `codedeploy-agent`; when the binary is launched
/// through that name we inject the `deploy-local` subcommand so the remaining
/// options parse unchanged.
///
/// When NOT invoked as `codedeploy-local`, argv is returned verbatim.
///
/// `-v`/`--version` is handled here rather than deferred to clap: the standalone
/// tool accepted it (the `deploy-local` subcommand does not, and the top-level
/// `-v` is consumed before subcommand dispatch), so we short-circuit and print
/// `codedeploy-local <version>` before exiting, matching the legacy output shape.
fn legacy_local_argv<I>(args: I) -> Vec<std::ffi::OsString>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    let mut args: Vec<std::ffi::OsString> = args.into_iter().collect();

    let invoked_as_local = args
        .first()
        .map(Path::new)
        // `file_stem` drops any extension so `codedeploy-local.exe` (Windows)
        // also matches; the comparison is case-insensitive for the same reason.
        .and_then(Path::file_stem)
        .is_some_and(|stem| stem.to_string_lossy().eq_ignore_ascii_case(LEGACY_LOCAL_PROGRAM));

    if !invoked_as_local {
        return args;
    }

    // Legacy `-v`/`--version`: the standalone tool printed its version and exited.
    if args[1..].iter().any(|a| a == "-v" || a == "--version") {
        println!("{LEGACY_LOCAL_PROGRAM} {}", env!("CARGO_PKG_VERSION"));
        process::exit(0);
    }

    // Inject the subcommand immediately after argv[0] so `codedeploy-local
    // --bundle-location …` becomes `codedeploy-local deploy-local
    // --bundle-location …`. Keeping argv[0] intact lets clap render usage with
    // the `codedeploy-local` program name.
    args.insert(1, std::ffi::OsString::from("deploy-local"));
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_start_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "start"]).unwrap();
        assert!(matches!(cli.command, Command::Start));
    }

    #[test]
    fn cli_parses_stop_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "stop"]).unwrap();
        assert!(matches!(cli.command, Command::Stop));
    }

    #[test]
    fn cli_parses_restart_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "restart"]).unwrap();
        assert!(matches!(cli.command, Command::Restart));
    }

    #[test]
    fn cli_parses_status_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "status"]).unwrap();
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn cli_parses_update_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "update"]).unwrap();
        assert!(matches!(cli.command, Command::Update));
    }

    #[test]
    fn cli_parses_hidden_worker_subcommand() {
        // clap strips the leading underscore: _worker variant -> "worker" subcommand
        let cli = Cli::try_parse_from(["codedeploy-agent", "worker"]).unwrap();
        assert!(matches!(cli.command, Command::_worker));
    }

    #[test]
    fn cli_rejects_unknown_subcommand() {
        assert!(Cli::try_parse_from(["codedeploy-agent", "unknown"]).is_err());
    }

    #[test]
    fn cli_parses_config_file_flag() {
        let cli = Cli::try_parse_from([
            "codedeploy-agent",
            "--config-file",
            "/tmp/test.yml",
            "start",
        ])
        .unwrap();
        assert_eq!(cli.config_file, Some(PathBuf::from("/tmp/test.yml")));
        assert!(matches!(cli.command, Command::Start));
    }

    #[test]
    fn cli_config_file_defaults_to_none() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "start"]).unwrap();
        assert!(cli.config_file.is_none());
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn tracker_path_joins_root_and_tracking_dir() {
        let config = AgentConfig::default();
        let path = tracker_path(&config);
        assert_eq!(path, PathBuf::from("/opt/codedeploy-agent/deployment-root/ongoing-deployment"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn tracker_path_respects_custom_config() {
        let config = AgentConfig {
            root_dir: PathBuf::from("/custom/root"),
            ongoing_deployment_tracking: "custom-tracking".to_string(),
            ..AgentConfig::default()
        };
        assert_eq!(tracker_path(&config), PathBuf::from("/custom/root/custom-tracking"));
    }

    #[test]
    fn make_master_config_maps_pid_dir() {
        let config =
            AgentConfig { pid_dir: PathBuf::from("/custom/pids"), ..AgentConfig::default() };
        let mc = make_master_config(&config);
        assert_eq!(mc.pid_dir, "/custom/pids");
    }

    #[test]
    fn make_master_config_maps_kill_wait() {
        let config =
            AgentConfig { kill_agent_max_wait_time_seconds: 999, ..AgentConfig::default() };
        let mc = make_master_config(&config);
        assert_eq!(mc.kill_wait_secs, 999);
    }

    #[test]
    fn make_master_config_maps_enable_command_port() {
        let config = AgentConfig { enable_command_port: true, ..AgentConfig::default() };
        let mc = make_master_config(&config);
        assert!(mc.enable_command_port);
    }

    #[test]
    fn make_master_config_maps_state_dir_from_pid_dir() {
        let config =
            AgentConfig { pid_dir: PathBuf::from("/custom/pids"), ..AgentConfig::default() };
        let mc = make_master_config(&config);
        assert_eq!(mc.state_dir, "/custom/pids");
    }

    #[test]
    fn parse_bundle_location_parses_s3_uri() {
        let parsed = parse_bundle_location("s3://my-bucket/path/to/bundle.zip").unwrap();
        assert_eq!(
            parsed,
            BundleLocation::S3 {
                bucket: "my-bucket".to_string(),
                key: "path/to/bundle.zip".to_string(),
            }
        );
    }

    #[test]
    fn parse_bundle_location_treats_path_as_local() {
        assert_eq!(parse_bundle_location("/tmp/bundle.tgz").unwrap(), BundleLocation::Local);
        assert_eq!(parse_bundle_location("relative/dir").unwrap(), BundleLocation::Local);
    }

    #[test]
    fn parse_bundle_location_rejects_s3_uri_without_key() {
        assert!(parse_bundle_location("s3://only-bucket").is_err());
    }

    #[test]
    fn parse_bundle_location_rejects_s3_uri_with_empty_bucket() {
        assert!(parse_bundle_location("s3:///key").is_err());
    }

    #[test]
    fn parse_bundle_location_rejects_s3_uri_with_empty_key() {
        assert!(parse_bundle_location("s3://bucket/").is_err());
    }

    fn gh(account: &str, repository: &str, commit_id: &str) -> BundleLocation {
        BundleLocation::GitHub {
            account: account.to_string(),
            repository: repository.to_string(),
            commit_id: commit_id.to_string(),
        }
    }

    #[test]
    fn parse_bundle_location_parses_github_browser_url_defaults_ref() {
        assert_eq!(
            parse_bundle_location("https://github.com/my-org/my-repo").unwrap(),
            gh("my-org", "my-repo", "HEAD")
        );
        // Trailing slash is tolerated.
        assert_eq!(
            parse_bundle_location("https://github.com/my-org/my-repo/").unwrap(),
            gh("my-org", "my-repo", "HEAD")
        );
        // `.git` suffix is stripped from the repository name.
        assert_eq!(
            parse_bundle_location("https://github.com/my-org/my-repo.git").unwrap(),
            gh("my-org", "my-repo", "HEAD")
        );
    }

    #[test]
    fn parse_bundle_location_parses_github_browser_url_with_ref() {
        assert_eq!(
            parse_bundle_location("https://github.com/my-org/my-repo/tree/main").unwrap(),
            gh("my-org", "my-repo", "main")
        );
        assert_eq!(
            parse_bundle_location("https://github.com/my-org/my-repo/commit/abc123").unwrap(),
            gh("my-org", "my-repo", "abc123")
        );
        // Branch names with slashes are preserved.
        assert_eq!(
            parse_bundle_location("https://github.com/my-org/my-repo/tree/feature/x").unwrap(),
            gh("my-org", "my-repo", "feature/x")
        );
    }

    #[test]
    fn parse_bundle_location_parses_github_api_url() {
        assert_eq!(
            parse_bundle_location("https://api.github.com/repos/my-org/my-repo/zipball/master")
                .unwrap(),
            gh("my-org", "my-repo", "master")
        );
        assert_eq!(
            parse_bundle_location("https://api.github.com/repos/my-org/my-repo/tarball/HEAD")
                .unwrap(),
            gh("my-org", "my-repo", "HEAD")
        );
        // A commit SHA as the ref, and a slash-containing branch ref.
        assert_eq!(
            parse_bundle_location("https://api.github.com/repos/my-org/my-repo/zipball/feature/x")
                .unwrap(),
            gh("my-org", "my-repo", "feature/x")
        );
        // A trailing slash (common on copy-paste) is trimmed from the ref.
        assert_eq!(
            parse_bundle_location("https://api.github.com/repos/my-org/my-repo/zipball/main/")
                .unwrap(),
            gh("my-org", "my-repo", "main")
        );
    }

    #[test]
    fn parse_bundle_location_rejects_malformed_github_urls() {
        // Browser URL missing the repository.
        assert!(parse_bundle_location("https://github.com/only-account").is_err());
        // API URL missing the ref.
        assert!(
            parse_bundle_location("https://api.github.com/repos/my-org/my-repo/zipball").is_err()
        );
        // API URL with an unknown archive segment.
        assert!(
            parse_bundle_location("https://api.github.com/repos/my-org/my-repo/blob/main").is_err()
        );
    }

    #[test]
    fn github_bundle_type_coercion() {
        // Only `tar` or `zip` are honored; everything else coerces to `zip`.
        assert_eq!(github_bundle_type("tar"), "tar");
        assert_eq!(github_bundle_type("zip"), "zip");
        // `tgz`, the `directory` default, and unknown values all become `zip`.
        assert_eq!(github_bundle_type("tgz"), "zip");
        assert_eq!(github_bundle_type("directory"), "zip");
        assert_eq!(github_bundle_type("rar"), "zip");
        assert_eq!(github_bundle_type(""), "zip");
    }

    #[test]
    fn parse_bundle_location_non_github_https_is_local() {
        // A non-GitHub https URL falls through to a local path (the pre-existing
        // fallthrough) and succeeds.
        assert_eq!(
            parse_bundle_location("https://example.com/bundle.zip").unwrap(),
            BundleLocation::Local
        );
    }

    #[test]
    fn choose_s3_region_prefers_onprem_region() {
        let region: Result<String, std::io::Error> =
            choose_s3_region("eu-west-1", || unreachable!("fallback must not run"));
        assert_eq!(region.unwrap(), "eu-west-1");
    }

    #[test]
    fn choose_s3_region_falls_back_when_onprem_empty() {
        let region = choose_s3_region("", || Ok::<_, std::io::Error>("us-west-2".to_string()));
        assert_eq!(region.unwrap(), "us-west-2");
    }

    #[test]
    fn choose_s3_region_propagates_fallback_error() {
        let region = choose_s3_region("", || Err::<String, _>(std::io::Error::other("no imds")));
        assert!(region.is_err());
    }

    #[test]
    fn cli_parses_deploy_local_with_s3_location() {
        let cli = Cli::try_parse_from([
            "codedeploy-agent",
            "deploy-local",
            "--bundle-location",
            "s3://my-bucket/path/bundle.zip",
            "--type",
            "zip",
        ])
        .unwrap();
        match cli.command {
            Command::DeployLocal { location, bundle_type, .. } => {
                assert_eq!(location, "s3://my-bucket/path/bundle.zip");
                assert_eq!(bundle_type, "zip");
            },
            _ => panic!("expected DeployLocal subcommand"),
        }
    }

    #[test]
    fn cli_parses_deploy_local_with_local_location() {
        let cli =
            Cli::try_parse_from(["codedeploy-agent", "deploy-local", "-l", "/tmp/bundle.tar"])
                .unwrap();
        match cli.command {
            Command::DeployLocal { location, .. } => {
                assert_eq!(location, "/tmp/bundle.tar");
            },
            _ => panic!("expected DeployLocal subcommand"),
        }
    }

    #[test]
    fn cli_deploy_local_defaults() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "deploy-local"]).unwrap();
        match cli.command {
            Command::DeployLocal {
                location,
                bundle_type,
                file_exists_behavior,
                deployment_group,
                deployment_group_name,
                application_name,
                appspec_filename,
                ..
            } => {
                assert_eq!(location, ".");
                assert_eq!(bundle_type, "directory");
                assert_eq!(file_exists_behavior, "DISALLOW");
                assert_eq!(deployment_group, "default-local-deployment-group");
                assert_eq!(deployment_group_name, "LocalFleet");
                assert_eq!(application_name, None);
                assert_eq!(appspec_filename, "appspec.yml");
            },
            _ => panic!("expected DeployLocal subcommand"),
        }
    }

    #[test]
    fn cli_deploy_local_parses_short_flags() {
        let cli = Cli::try_parse_from([
            "codedeploy-agent",
            "deploy-local",
            "-l",
            "s3://b/k.tgz",
            "-t",
            "tgz",
            "-b",
            "OVERWRITE",
            "-g",
            "grp",
            "-d",
            "Fleet",
            "-a",
            "MyApp",
            "-e",
            "BeforeInstall,AfterInstall",
            "-c",
            "/tmp/c.yml",
            "-A",
            "custom.yml",
        ])
        .unwrap();
        match cli.command {
            Command::DeployLocal {
                location,
                bundle_type,
                file_exists_behavior,
                deployment_group,
                deployment_group_name,
                application_name,
                events,
                agent_configuration_file,
                appspec_filename,
                github_token,
            } => {
                assert_eq!(location, "s3://b/k.tgz");
                assert_eq!(bundle_type, "tgz");
                assert_eq!(file_exists_behavior, "OVERWRITE");
                assert_eq!(deployment_group, "grp");
                assert_eq!(deployment_group_name, "Fleet");
                assert_eq!(application_name.as_deref(), Some("MyApp"));
                assert_eq!(
                    events.unwrap(),
                    vec!["BeforeInstall".to_string(), "AfterInstall".to_string()]
                );
                assert_eq!(agent_configuration_file, Some(PathBuf::from("/tmp/c.yml")));
                assert_eq!(appspec_filename, "custom.yml");
                assert_eq!(github_token, None);
            },
            _ => panic!("expected DeployLocal subcommand"),
        }
    }

    #[test]
    fn cli_deploy_local_parses_github_token() {
        let cli = Cli::try_parse_from([
            "codedeploy-agent",
            "deploy-local",
            "-l",
            "https://github.com/my-org/my-repo",
            "--github-token",
            "ghp_secret",
        ])
        .unwrap();
        match cli.command {
            Command::DeployLocal { github_token, .. } => {
                assert_eq!(github_token.as_deref(), Some("ghp_secret"));
            },
            _ => panic!("expected DeployLocal subcommand"),
        }
    }

    /// Helper: run `legacy_local_argv` on a `&str` argv and return the result as
    /// `Vec<String>` for easy assertions.
    fn legacy_argv(args: &[&str]) -> Vec<String> {
        legacy_local_argv(args.iter().map(std::ffi::OsString::from))
            .into_iter()
            .map(|s| s.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn legacy_argv_injects_deploy_local_when_invoked_as_codedeploy_local() {
        // A full path is realistic (the installed symlink) and exercises basename
        // extraction.
        let out = legacy_argv(&[
            "/opt/codedeploy-agent/bin/codedeploy-local",
            "--bundle-location",
            "/tmp/bundle",
            "--type",
            "directory",
        ]);
        assert_eq!(
            out,
            vec![
                "/opt/codedeploy-agent/bin/codedeploy-local",
                "deploy-local",
                "--bundle-location",
                "/tmp/bundle",
                "--type",
                "directory",
            ]
        );
    }

    #[test]
    fn legacy_argv_matches_windows_exe_name() {
        // `.exe` suffix (Windows) must still match via `file_stem`. Backslash
        // separators are only split on Windows, so this test uses the bare
        // filename to stay host-independent.
        let out = legacy_argv(&["codedeploy-local.exe", "-l", "."]);
        assert_eq!(out[1], "deploy-local");
    }

    #[test]
    fn legacy_argv_left_untouched_for_agent_binary() {
        let out = legacy_argv(&["codedeploy-agent", "start"]);
        assert_eq!(out, vec!["codedeploy-agent", "start"]);
    }

    #[test]
    fn legacy_argv_does_not_inject_for_agent_deploy_local() {
        // Invoked under the agent name, the user already typed the subcommand;
        // we must not double-inject it.
        let out = legacy_argv(&["codedeploy-agent", "deploy-local", "-l", "."]);
        assert_eq!(out, vec!["codedeploy-agent", "deploy-local", "-l", "."]);
    }

    #[test]
    fn legacy_argv_injected_form_parses_as_deploy_local() {
        // End-to-end: the rewritten argv must parse into DeployLocal with flags
        // carried through unchanged.
        let out = legacy_local_argv(
            ["codedeploy-local", "-l", "s3://b/k.tgz", "-t", "tgz"]
                .into_iter()
                .map(std::ffi::OsString::from),
        );
        let cli = Cli::try_parse_from(out).unwrap();
        match cli.command {
            Command::DeployLocal { location, bundle_type, .. } => {
                assert_eq!(location, "s3://b/k.tgz");
                assert_eq!(bundle_type, "tgz");
            },
            _ => panic!("expected DeployLocal subcommand"),
        }
    }

    fn sv(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn ordered_lifecycle_events_defaults_when_none() {
        assert_eq!(ordered_lifecycle_events(None), sv(DEFAULT_ORDERED_LIFECYCLE_EVENTS));
    }

    #[test]
    fn ordered_lifecycle_events_empty_uses_default() {
        assert_eq!(ordered_lifecycle_events(Some(&[])), sv(DEFAULT_ORDERED_LIFECYCLE_EVENTS));
    }

    #[test]
    fn ordered_lifecycle_events_passes_through_user_events() {
        let custom = sv(&["HealthCheck", "AfterInstall"]);
        assert_eq!(ordered_lifecycle_events(Some(&custom)), custom);
    }

    #[test]
    fn add_required_events_injects_when_missing() {
        let seq = add_download_bundle_and_install_events(&sv(&["AfterInstall"]));
        assert_eq!(seq, sv(&["DownloadBundle", "Install", "AfterInstall"]));
    }

    #[test]
    fn add_required_events_swaps_install_before_download_bundle() {
        let seq = add_download_bundle_and_install_events(&sv(&["Install", "DownloadBundle"]));
        let dl = seq.iter().position(|e| e == "DownloadBundle").unwrap();
        let install = seq.iter().position(|e| e == "Install").unwrap();
        assert!(dl < install, "DownloadBundle must precede Install: {seq:?}");
    }

    #[test]
    fn command_sequence_default_orders_required_events() {
        let seq = build_local_command_sequence(None);
        let dl = seq.iter().position(|e| e == "DownloadBundle").unwrap();
        let install = seq.iter().position(|e| e == "Install").unwrap();
        assert!(dl < install);
        assert_eq!(seq.first().unwrap(), "BeforeBlockTraffic");
    }

    #[test]
    fn command_sequence_schedules_custom_event() {
        let seq = build_local_command_sequence(Some(&sv(&["BeforeInstall", "HealthCheck"])));
        assert_eq!(seq, sv(&["DownloadBundle", "Install", "BeforeInstall", "HealthCheck"]));
    }

    #[test]
    fn hook_mapping_includes_custom_event_and_excludes_required() {
        let mapping = build_local_hook_mapping(Some(&sv(&["HealthCheck"])));
        assert_eq!(mapping["HealthCheck"], vec!["HealthCheck".to_string()]);
        assert!(!mapping.contains_key("DownloadBundle"));
        assert!(!mapping.contains_key("Install"));
        assert!(mapping.contains_key("AfterInstall")); // default events retained
    }

    #[test]
    fn validate_event_ordering_accepts_valid_and_custom_orders() {
        assert!(
            validate_event_ordering(&sv(&["ApplicationStop", "DownloadBundle", "Install"])).is_ok()
        );
        assert!(
            validate_event_ordering(&sv(&["HealthCheck", "DownloadBundle", "Install"])).is_ok()
        );
        assert!(
            validate_event_ordering(&sv(&["DownloadBundle", "BeforeInstall", "Install"])).is_ok()
        );
    }

    #[test]
    fn validate_event_ordering_rejects_bad_download_bundle_order() {
        assert!(
            validate_event_ordering(&sv(&["AfterInstall", "DownloadBundle"]))
                .unwrap_err()
                .contains("before DownloadBundle")
        );
        assert!(
            validate_event_ordering(&sv(&["Install", "DownloadBundle"]))
                .unwrap_err()
                .contains("before DownloadBundle")
        );
    }

    #[test]
    fn validate_event_ordering_rejects_new_revision_before_install() {
        let err = validate_event_ordering(&sv(&["DownloadBundle", "AfterInstall", "Install"]))
            .unwrap_err();
        assert!(err.contains("before Install"), "got: {err}");
    }
}
