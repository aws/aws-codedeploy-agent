//! @risk medium
//!
//! Worker child process.
//!
//! Each worker is a child process spawned by the master that runs the
//! polling loop via `HostCommandPoller`.

use std::process;
use std::sync::Arc;

use tracing::{error, info};

use super::signal::ShutdownFlag;
use crate::aws_clients::codedeploy_command_client::{
    CodeDeployClientError, CodeDeployCommandClient,
};
use crate::aws_clients::credentials::Credentials;
use crate::command_poller::{CancelToken, CommandProcessor, HostCommandPoller};
use crate::config::{AgentConfig, resolve_region};
use crate::host_command::CommandDispatcher;
use crate::host_command::DeploymentArchives;
use crate::host_command::commands::hook::HookMapping;
use crate::runtime::FileBasedDeploymentTracker;
use crate::system::SystemFileOperations;

/// Worker entry point: resolve credentials, start polling.
///
/// Initializes AWS clients, creates the command pipeline, and enters
/// the polling loop via `HostCommandPoller`.
///
/// # Call site
/// Called from the `worker` subprocess entry point in `main.rs`, never
/// in-process by the master. The `shutdown` parameter is owned by the
/// subprocess and accepted (rather than created internally) so unit tests
/// can inject their own flag.
///
/// Config and logging are initialized by `main.rs` before calling this
/// function. The worker receives a pre-loaded `AgentConfig` to avoid
/// re-loading from disk (the master already validated it).
pub fn run(shutdown: &ShutdownFlag, config: &AgentConfig) {
    let pid = process::id();
    info!("Worker {pid} starting");

    // Resolve credentials.
    let mut credentials = match Credentials::load(&config.on_premises_config_file) {
        Ok(c) => c,
        Err(e) => {
            error!("Worker failed to resolve credentials: {e}");
            return;
        },
    };

    // Resolve region when not provided by on-premises config (InstanceProfile mode).
    // Ruby: `onpremise_config.rb` sets `ENV['AWS_REGION']` from the on-premises file,
    // then `config.rb#region` reads `ENV['AWS_REGION'] || InstanceMetadata.region`.
    // For InstanceProfile, credentials.region is empty — resolve via env/IMDS.
    if credentials.region.is_empty() {
        match resolve_region(config.disable_imds_v1) {
            Ok(region) => {
                info!("Resolved region: {region}");
                credentials.region = region;
            },
            Err(e) => {
                error!("Failed to resolve AWS region: {e}");
                return;
            },
        }
    }
    let host_identifier = credentials.host_identifier.clone();

    // Validate TLS connectivity to the CodeDeploy endpoint before entering the polling loop.
    // Ruby: `validate_ssl_config` in `codedeploy_control.rb` — aborts the agent if TLS fails.
    let endpoint = codedeploy_commands::endpoint::resolve(
        &credentials.region,
        config.deploy_control_endpoint.as_deref(),
        config.use_fips_mode,
        config.enable_auth_policy,
    );
    if let Err(e) =
        crate::aws_clients::ssl::verify_tls_connection(&endpoint, config.proxy_uri.as_deref())
    {
        error!("SSL validation failed for endpoint {endpoint}: {e}");
        return;
    }
    info!("TLS verification passed for {endpoint}");

    // Create CodeDeploy clients.
    // Two clients needed: HostCommandPoller owns one, CommandProcessor owns the other.
    // Ruby uses a single shared client; Rust ownership requires separate instances.
    // Both are stateless HTTP clients — safe to create two with the same config.
    let http_timeout = std::time::Duration::from_secs(config.http_read_timeout);
    let client = match create_client(config, &credentials, http_timeout) {
        Ok(c) => c,
        Err(e) => {
            error!("Worker failed to create CodeDeploy client: {e}");
            return;
        },
    };
    let s3_credentials = credentials.clone();
    let processor_client = match create_client(config, &credentials, http_timeout) {
        Ok(c) => c,
        Err(e) => {
            error!("Worker failed to create processor client: {e}");
            return;
        },
    };

    // Build deployment infrastructure.
    let deployment_root = config.root_dir.clone();
    let instructions_dir = config.root_dir.join("deployment-instructions");
    let archives = Arc::new(DeploymentArchives::new(
        deployment_root,
        instructions_dir,
        config.max_revisions as usize,
    ));
    let s3_config = crate::aws_clients::s3_client::S3ClientConfig {
        use_fips: config.use_fips_mode,
        proxy_uri: config.proxy_uri.clone(),
        ..Default::default()
    };
    let s3_client = match crate::aws_clients::s3_client::S3Client::new(s3_credentials, &s3_config) {
        Ok(c) => c,
        Err(e) => {
            error!("Worker failed to create S3 client: {e}");
            return;
        },
    };
    let dispatcher = CommandDispatcher::new(archives, Some(s3_client), default_hook_mapping());
    // Two trackers needed: HostCommandPoller and CommandProcessor each take ownership.
    // Both point at the same path — safe today because FileBasedDeploymentTracker is
    // stateless (reads/writes on each call, no cached state or file locks).
    // Two trackers needed: poller owns one, processor owns the other.
    // Revisit if tracker gains internal state that needs sharing.
    let tracker_path = config.root_dir.join(&config.ongoing_deployment_tracking);
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(tracker_path.clone());
    let processor_tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(tracker_path);
    let processor = CommandProcessor::new(
        processor_client,
        dispatcher,
        processor_tracker,
        host_identifier.clone(),
    );

    // Bridge ShutdownFlag → CancelToken.
    // Bridge thread is intentionally not joined — it exits when the process does.
    let cancel = CancelToken::new();
    let cancel_clone = cancel.clone();
    let shutdown_clone = shutdown.clone();
    std::thread::spawn(move || {
        while !shutdown_clone.is_set() {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        cancel_clone.cancel();
    });

    // Start polling.
    let poller = HostCommandPoller::new(client, processor, tracker, host_identifier, cancel)
        .with_poll_interval(std::time::Duration::from_secs(config.wait_between_runs))
        .with_shutdown_timeout(std::time::Duration::from_secs(
            config.kill_agent_max_wait_time_seconds,
        ));

    info!("Worker {pid} entering polling loop");
    poller.start();
    info!("Worker {pid} shutting down");
}

fn create_client(
    config: &AgentConfig,
    credentials: &Credentials,
    http_timeout: std::time::Duration,
) -> Result<CodeDeployCommandClient, CodeDeployClientError> {
    CodeDeployCommandClient::new(
        credentials.clone(),
        config.use_fips_mode,
        config.enable_auth_policy,
        config.deploy_control_endpoint.clone(),
        http_timeout,
        config.proxy_uri.clone(),
    )
}

/// Standard `CodeDeploy` hook command mapping.
///
/// Ruby: `command_executor.rb` — each command name maps to itself as a
/// lifecycle event. `DownloadBundle` and `Install` are handled separately
/// by the dispatcher, not as hooks.
fn default_hook_mapping() -> HookMapping {
    [
        "ApplicationStop",
        "BeforeBlockTraffic",
        "AfterBlockTraffic",
        "BeforeInstall",
        "AfterInstall",
        "ApplicationStart",
        "BeforeAllowTraffic",
        "AfterAllowTraffic",
        "ValidateService",
    ]
    .into_iter()
    .map(|name| (name.to_string(), vec![name.to_string()]))
    .collect()
}

/// Spawn a worker as a child process by re-executing the current binary
/// with an internal `_worker` subcommand.
///
/// # Errors
/// Returns an error if the child process cannot be spawned.
pub fn spawn() -> std::io::Result<process::Child> {
    let exe = std::env::current_exe()?;
    let child = process::Command::new(exe).arg("worker").spawn()?;
    info!("Spawned worker child process (pid {})", child.id());
    Ok(child)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_exits_when_shutdown_set() {
        let flag = ShutdownFlag::new();
        let flag_clone = flag.clone();
        let config = AgentConfig::default();
        let handle = std::thread::spawn(move || {
            run(&flag_clone, &config);
        });
        // Give the worker a moment to start
        std::thread::sleep(std::time::Duration::from_millis(50));
        flag.set();
        handle.join().unwrap();
    }

    #[test]
    fn default_hook_mapping_contains_all_lifecycle_events() {
        let mapping = default_hook_mapping();
        let expected = [
            "ApplicationStop",
            "BeforeBlockTraffic",
            "AfterBlockTraffic",
            "BeforeInstall",
            "AfterInstall",
            "ApplicationStart",
            "BeforeAllowTraffic",
            "AfterAllowTraffic",
            "ValidateService",
        ];
        assert_eq!(mapping.len(), expected.len());
        for event in &expected {
            assert!(mapping.contains_key(*event), "missing hook mapping for {event}");
        }
    }

    #[test]
    #[ignore = "requires compiled binary with _worker subcommand"]
    fn spawn_creates_child_process() {
        let mut child = spawn().expect("spawn should succeed with full binary");
        let _ = child.kill();
        let _ = child.wait();
    }
}
