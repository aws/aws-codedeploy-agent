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
use crate::config::{
    AgentConfig, resolve_host_identifier, resolve_region, resolve_region_and_host_identifier,
};
use crate::host_command::CommandDispatcher;
use crate::host_command::DeploymentArchives;
use crate::host_command::commands::hook::HookMapping;
use crate::runtime::FileBasedDeploymentTracker;
use crate::system::SystemFileOperations;

/// Reset the worker's process umask to the standard `0o022` before customer
/// bundle extraction.
///
/// Customer archive entries with no stored Unix mode (e.g. Windows/FAT zips)
/// take the umask default; `0o022` lands them at 0644 so the service user can
/// read them. Agent files set explicit modes and are unaffected.
#[cfg(unix)]
fn reset_umask_for_customer_files() {
    use nix::sys::stat::{Mode, umask};
    umask(Mode::from_bits_truncate(0o022));
}

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
#[allow(clippy::too_many_lines)]
pub fn run(shutdown: &ShutdownFlag, config: &AgentConfig) {
    let pid = process::id();
    info!("Worker {pid} starting");

    // The worker extracts customer bundles; archive entries with no stored Unix
    // mode (e.g. zipped on Windows/FAT) take the umask default. Reset to 0o022 so
    // those files land at 0644 and stay readable by the service user; agent files
    // keep their explicit restrictive modes.
    #[cfg(unix)]
    reset_umask_for_customer_files();

    // Resolve credentials.
    // GRCOV_STOP_COVERAGE
    let mut credentials = match Credentials::load(&config.on_premises_config_file) {
        Ok(c) => c,
        Err(e) => {
            error!("Worker failed to resolve credentials: {e}");
            return;
        },
    };
    // GRCOV_BEGIN_COVERAGE

    // Resolve region and host identifier when not provided by on-premises config
    // (InstanceProfile mode). Uses a single IMDS identity document fetch for both.
    if credentials.region.is_empty() || credentials.host_identifier.is_empty() {
        // GRCOV_STOP_COVERAGE
        if credentials.region.is_empty() && credentials.host_identifier.is_empty() {
            // Both need resolving — use combined function for single IMDS fetch.
            match resolve_region_and_host_identifier(config.disable_imds_v1) {
                Ok((region, host_id)) => {
                    info!("Resolved region: {region}");
                    info!("Resolved host identifier: {host_id}");
                    credentials.region = region;
                    credentials.host_identifier = host_id;
                },
                Err(e) => {
                    error!("Failed to resolve region/host identifier: {e}");
                    return;
                },
            }
        } else if credentials.region.is_empty() {
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
        } else {
            match resolve_host_identifier(config.disable_imds_v1) {
                Ok(id) => {
                    info!("Resolved host identifier: {id}");
                    credentials.host_identifier = id;
                },
                Err(e) => {
                    error!("Failed to resolve host identifier: {e}");
                    return;
                },
            }
        }
    }
    // GRCOV_BEGIN_COVERAGE

    let host_identifier = credentials.host_identifier.clone();

    // Validate TLS connectivity to the CodeDeploy endpoint before entering the polling loop.
    let endpoint = codedeploy_commands::endpoint::resolve(
        &credentials.region,
        config.deploy_control_endpoint.as_deref(),
        config.use_fips_mode,
        config.enable_auth_policy,
    );
    // GRCOV_STOP_COVERAGE
    if let Err(e) =
        crate::aws_clients::ssl::verify_tls_connection(&endpoint, config.proxy_uri.as_deref())
    {
        error!("SSL validation failed for endpoint {endpoint}: {e}");
        return;
    }
    info!("TLS verification passed for {endpoint}");
    // GRCOV_BEGIN_COVERAGE

    // Create CodeDeploy clients.
    // Two clients needed: HostCommandPoller owns one, CommandProcessor owns the other.
    // Both are stateless HTTP clients — safe to create two with the same config.
    // They SHARE a ThrottleGate so a 429 on any thread backs off all threads.
    let http_timeout = std::time::Duration::from_secs(config.http_read_timeout);
    let throttle_gate = std::sync::Arc::new(crate::aws_clients::ThrottleGate::new());
    // GRCOV_STOP_COVERAGE
    let client = match create_client_with_gate(
        config,
        &credentials,
        http_timeout,
        std::sync::Arc::clone(&throttle_gate),
    ) {
        Ok(c) => c,
        Err(e) => {
            error!("Worker failed to create CodeDeploy client: {e}");
            return;
        },
    };
    let s3_credentials = credentials.clone();
    let processor_client = match create_client_with_gate(
        config,
        &credentials,
        http_timeout,
        std::sync::Arc::clone(&throttle_gate),
    ) {
        Ok(c) => c,
        Err(e) => {
            error!("Worker failed to create processor client: {e}");
            return;
        },
    };

    // Build deployment infrastructure.
    let deployment_root = config.root_dir.clone();
    let instructions_dir = config.root_dir.join("deployment-instructions");
    let archives = Arc::new(
        DeploymentArchives::new(deployment_root, instructions_dir, config.max_revisions as usize)
            .with_restrict_permissions(config.hardening.restrict_agent_dir_permissions),
    );
    let s3_config = crate::aws_clients::s3_client::S3ClientConfig {
        use_fips: config.use_fips_mode,
        proxy_uri: config.proxy_uri.clone(),
        wire_log: config
            .log_aws_wire
            .then(|| (config.log_dir.clone(), config.program_name.clone())),
        ..Default::default()
    };
    // GRCOV_STOP_COVERAGE
    let s3_client = match crate::aws_clients::s3_client::S3Client::new(s3_credentials, &s3_config) {
        Ok(c) => c,
        Err(e) => {
            error!("Worker failed to create S3 client: {e}");
            return;
        },
    };
    // S3Client is not Clone (tokio Runtime is not Clone), so we create a
    // separate client for the update command. Both use the same credentials
    // and config.
    let update_s3_client =
        match crate::aws_clients::s3_client::S3Client::new(credentials.clone(), &s3_config) {
            Ok(c) => c,
            Err(e) => {
                error!("Worker failed to create update S3 client: {e}");
                return;
            },
        };
    let region = credentials.region.clone();
    let dispatcher = CommandDispatcher::new(
        archives,
        Some(s3_client),
        default_hook_mapping(),
        &region,
        Some(update_s3_client),
        Arc::new(config.clone()),
    );
    // Two trackers needed: HostCommandPoller and CommandProcessor each take ownership.
    // Both point at the same path — safe today because FileBasedDeploymentTracker is
    // stateless (reads/writes on each call, no cached state or file locks).
    // Two trackers needed: poller owns one, processor owns the other.
    // Revisit if tracker gains internal state that needs sharing.
    let tracker_path = config.root_dir.join(&config.ongoing_deployment_tracking);
    let tracker_ops =
        SystemFileOperations::with_policy(config.hardening.restrict_agent_dir_permissions);
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new_with_ops(
        tracker_path.clone(),
        tracker_ops,
    );
    let processor_tracker =
        FileBasedDeploymentTracker::<SystemFileOperations>::new_with_ops(tracker_path, tracker_ops);
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
    let mut poller = HostCommandPoller::new(client, processor, tracker, host_identifier, cancel)
        .with_poll_interval(std::time::Duration::from_secs(config.wait_between_runs))
        .with_shutdown_timeout(std::time::Duration::from_secs(
            config.kill_agent_max_wait_time_seconds,
        ));
    // Wire the command-port inject directory so this poll loop consumes the
    // commands the master's command port writes into the pid/state dir.
    if config.enable_command_port {
        poller = poller.with_inject_dir(config.pid_dir.clone());
    }

    info!("Worker {pid} entering polling loop");
    poller.start();
    info!("Worker {pid} shutting down");
}
// GRCOV_BEGIN_COVERAGE

fn create_client_with_gate(
    config: &AgentConfig,
    credentials: &Credentials,
    http_timeout: std::time::Duration,
    throttle_gate: std::sync::Arc<crate::aws_clients::ThrottleGate>,
) -> Result<CodeDeployCommandClient, CodeDeployClientError> {
    CodeDeployCommandClient::with_throttle_gate(
        credentials.clone(),
        config.use_fips_mode,
        config.enable_auth_policy,
        config.deploy_control_endpoint.clone(),
        http_timeout,
        config.proxy_uri.clone(),
        throttle_gate,
    )
}

/// Standard `CodeDeploy` hook command mapping.
///
/// Each command name maps to itself as a lifecycle event. `DownloadBundle`
/// and `Install` are handled separately by the dispatcher, not as hooks.
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

// GRCOV_STOP_COVERAGE
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

/// Tie this worker's lifetime to the master via `PR_SET_PDEATHSIG=SIGKILL`: the
/// kernel kills the worker when its parent dies. A `SIGKILL`ed master runs no
/// shutdown path, and under systemd's `KillMode=control-group` the service cgroup
/// stays open until `TimeoutStopSec`; binding the lifetime lets the worker exit
/// with the master and release the cgroup promptly.
///
/// Called by the worker (not via parent `pre_exec`) because this crate forbids
/// `unsafe` on Unix and the `nix` prctl wrapper is safe. `PR_SET_PDEATHSIG` fires
/// only on a *future* parent death, so the `getppid() == 1` re-check covers the
/// window where the master already died and the worker was reparented to init.
#[cfg(target_os = "linux")]
pub fn bind_lifetime_to_parent() {
    use nix::sys::prctl;
    use nix::sys::signal::Signal;
    use nix::unistd::{Pid, getppid};

    if let Err(e) = prctl::set_pdeathsig(Signal::SIGKILL) {
        error!("Worker failed to set PR_SET_PDEATHSIG; master death may orphan it: {e}");
        return;
    }
    if getppid() == Pid::from_raw(1) {
        info!("Master already exited before worker startup; exiting to avoid orphan");
        process::exit(0);
    }
}

/// No-op on non-Linux: `PR_SET_PDEATHSIG` is Linux-specific.
#[cfg(not(target_os = "linux"))]
pub fn bind_lifetime_to_parent() {}
// GRCOV_BEGIN_COVERAGE

#[cfg(test)]
mod tests {
    use super::*;

    /// The worker must reset the umask to 0o022 so no-stored-mode customer files
    /// land at 0644.
    #[cfg(unix)]
    #[test]
    #[serial_test::serial(umask)]
    fn worker_run_resets_umask_to_0022() {
        use nix::sys::stat::{Mode, umask};

        // Simulate the master's restrictive umask.
        let prev = umask(Mode::from_bits_truncate(0o027));

        reset_umask_for_customer_files();

        // Read back the current umask (umask() returns the previous value).
        let now = umask(Mode::from_bits_truncate(0o022));
        // Restore whatever was there before the test.
        umask(prev);

        assert_eq!(now, Mode::from_bits_truncate(0o022), "worker must reset umask to 0o022");
    }

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
