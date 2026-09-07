//! Windows Service Control Manager integration.
//!
//! Implements the documented service name `codedeployagent` and display name
//! `CodeDeploy Host Agent Service`.

use std::ffi::OsString;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::config::AgentConfig;
use crate::daemon::signal::ShutdownFlag;
use crate::daemon::worker;
use crate::logging::{LogConfig, init_logging};
use crate::paths;

/// Windows service name registered with the Service Control Manager.
const SERVICE_NAME: &str = "codedeployagent";

/// Display name shown in `services.msc`.
const SERVICE_DISPLAY_NAME: &str = "CodeDeploy Host Agent Service";

/// Service type: standalone process.
const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;

/// Documented AWS CodeDeploy agent install path on Windows (see
/// <https://docs.aws.amazon.com/codedeploy/latest/userguide/codedeploy-agent-operations-install-windows.html>).
/// The MSI creates this directory with an admin-only ACL, so a non-admin
/// local user cannot replace the binary and escalate to LocalSystem.
///
/// Uppercased for case-insensitive comparison against a canonicalized
/// path, which also resolves symlinks, junctions, and 8.3 short names.
const ALLOWED_INSTALL_PREFIX: &str = r"C:\PROGRAMDATA\AMAZON\CODEDEPLOY\";

/// Reject `exe_path` if it is not under [`ALLOWED_INSTALL_PREFIX`].
///
/// Registering a LocalSystem service whose binary lives in a user-writable
/// directory (e.g. `C:\Users\...\Downloads`) is a local privilege
/// escalation: any local user who can write the file can replace it and
/// gain SYSTEM on the next service start.
///
/// Returns the canonicalized path so callers can register that with SCM
/// instead of the original `exe_path`. Registering the original would
/// reintroduce a TOCTOU: if `exe_path` is a junction/symlink that points
/// inside the allowed prefix at check time, an attacker who can later
/// repoint it would gain SYSTEM on next service start. The allowed prefix is
/// the protected install root (`%PROGRAMDATA%\Amazon\CodeDeploy`).
fn validate_service_binary_path(exe_path: &std::path::Path) -> std::io::Result<std::path::PathBuf> {
    let canonical = exe_path.canonicalize()?;
    let canonical_str = canonical.to_str().ok_or_else(|| {
        std::io::Error::other(format!(
            "Service binary path contains non-Unicode characters: {}",
            canonical.display(),
        ))
    })?;
    let canonical_upper = canonical_str.to_uppercase();
    // Strip Windows canonicalize()'s `\\?\` verbatim prefix so the match
    // lines up with the human-readable path.
    let stripped = canonical_upper.strip_prefix(r"\\?\").unwrap_or(&canonical_upper);
    if stripped.starts_with(ALLOWED_INSTALL_PREFIX) {
        return Ok(canonical);
    }
    Err(std::io::Error::other(format!(
        "Refusing to register service binary from a potentially writable \
         location: {}. Install under {} first.",
        canonical.display(),
        ALLOWED_INSTALL_PREFIX,
    )))
}

/// Install the agent as a Windows service.
///
/// Registers the service with the Service Control Manager so it can be
/// started via `sc start codedeployagent` or the Services snap-in. The
/// binary path is validated against [`ALLOWED_INSTALL_PREFIX`] first to
/// prevent local privilege escalation via a user-writable binary path.
pub fn install() -> std::io::Result<()> {
    let exe_path = std::env::current_exe()?;
    let canonical_exe = validate_service_binary_path(&exe_path)?;

    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

    let info = ServiceInfo {
        name: OsString::from(SERVICE_NAME),
        display_name: OsString::from(SERVICE_DISPLAY_NAME),
        service_type: SERVICE_TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: canonical_exe,
        launch_arguments: vec![OsString::from("run-as-service")],
        dependencies: vec![],
        // `account_name: None` registers the service as LocalSystem.
        // The agent deploys to arbitrary customer-chosen locations
        // (Program Files, service dirs, HKLM, other users' trees) and restarts
        // services, all of which require SYSTEM; a lower-privilege account would
        // break those deployments. The binary-swap escalation vector this opens
        // is closed by `validate_service_binary_path` above.
        account_name: None,
        account_password: None,
    };

    manager
        .create_service(&info, ServiceAccess::empty())
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(())
}

/// Uninstall the agent Windows service.
///
/// Removes the service registration from the Service Control Manager.
/// If the service is running, sends a stop control and waits (up to
/// 30 seconds) for it to reach the Stopped state before deleting.
/// Without this, `delete()` merely marks a running service for deletion
/// and it remains visible until it stops on its own.
///
/// Returns an error if the service does not exist or does not stop
/// within the timeout.
pub fn uninstall() -> std::io::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let service = manager
        .open_service(
            SERVICE_NAME,
            ServiceAccess::DELETE | ServiceAccess::STOP | ServiceAccess::QUERY_STATUS,
        )
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    // Stop the service first if it is running, so delete() actually removes
    // it rather than just marking it for deletion. Separate "send stop"
    // from "wait for stopped" so that StopPending (another admin already
    // issued a stop) is handled gracefully instead of erroring out of
    // service.stop().
    let status = service.query_status().map_err(|e| std::io::Error::other(e.to_string()))?;
    if status.current_state != ServiceState::Stopped
        && status.current_state != ServiceState::StopPending
    {
        service.stop().map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    if status.current_state != ServiceState::Stopped {
        let timeout = Duration::from_secs(30);
        let start = std::time::Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(500));
            let s = service.query_status().map_err(|e| std::io::Error::other(e.to_string()))?;
            if s.current_state == ServiceState::Stopped {
                break;
            }
            if start.elapsed() > timeout {
                return Err(std::io::Error::other("Timed out waiting for service to stop"));
            }
        }
    }

    service.delete().map_err(|e| std::io::Error::other(e.to_string()))
}

define_windows_service!(ffi_service_main, service_main);

/// Outcome of attempting to start as a Windows service.
#[derive(Debug)]
pub enum DispatchOutcome {
    /// `StartServiceCtrlDispatcher` succeeded — the service ran under SCM
    /// and has now stopped cleanly.
    RanAsService,
    /// The process was not started by the Service Control Manager.
    /// `StartServiceCtrlDispatcher` returned
    /// `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT` (1063). Caller should
    /// fall back to console mode.
    NotLaunchedByScm,
}

/// Windows error code returned by `StartServiceCtrlDispatcher` when the
/// process was not launched by the Service Control Manager.
///
/// Documented in MSDN as `ERROR_FAILED_SERVICE_CONTROLLER_CONNECT`.
const ERROR_FAILED_SERVICE_CONTROLLER_CONNECT: i32 = 1063;

/// Attempt to start the Windows service dispatcher.
///
/// Returns [`DispatchOutcome::RanAsService`] if `StartServiceCtrlDispatcher`
/// succeeds (blocks until service stops), or [`DispatchOutcome::NotLaunchedByScm`]
/// if the process was not launched by SCM (error 1063). Other errors are
/// propagated as `io::Error`.
pub fn try_run() -> std::io::Result<DispatchOutcome> {
    match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
        Ok(()) => Ok(DispatchOutcome::RanAsService),
        Err(windows_service::Error::Winapi(ref e))
            if e.raw_os_error() == Some(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT) =>
        {
            Ok(DispatchOutcome::NotLaunchedByScm)
        },
        Err(windows_service::Error::Winapi(e)) => Err(e),
        Err(e) => Err(std::io::Error::other(e.to_string())),
    }
}

/// Run the agent as a Windows service. **Must be called by SCM.**
///
/// If invoked outside an SCM context, returns an error explaining that
/// the caller should use `_worker` (or no subcommand) for console mode.
pub fn run() -> std::io::Result<()> {
    outcome_to_result(try_run()?)
}

/// Map a dispatch outcome to `run()`'s result. Split out so tests can
/// exercise the error contract without calling the service dispatcher,
/// which Windows allows only once per process.
fn outcome_to_result(outcome: DispatchOutcome) -> std::io::Result<()> {
    match outcome {
        DispatchOutcome::RanAsService => Ok(()),
        DispatchOutcome::NotLaunchedByScm => Err(std::io::Error::other(
            "`run-as-service` must be invoked by the Windows Service Control \
             Manager. To run the worker in a console for debugging, use \
             `codedeploy-agent _worker` instead.",
        )),
    }
}

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_service() {
        // Write bootstrap error before logging is available. Append rather
        // than truncate so a restart loop preserves every failure. The error
        // text goes only to the agent-owned log dir (under the protected install
        // root on Windows), never to a remote or customer-visible surface.
        let log_dir = paths::log_dir();
        let _ = std::fs::create_dir_all(&log_dir);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("service-bootstrap.log"))
        {
            use std::io::Write;
            let _ = writeln!(f, "[{:?}] {e}", std::time::SystemTime::now());
        }
    }
}

fn run_service() -> std::io::Result<()> {
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let flag_clone = Arc::clone(&shutdown_flag);

    let event_handler = move |control: ServiceControl| -> ServiceControlHandlerResult {
        match control {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                ServiceControlHandlerResult::NoError
            },
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    };

    let status_handle = service_control_handler::register(SERVICE_NAME, event_handler)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    // Report StartPending while we initialize (load config, create dirs,
    // set up logging). controls_accepted is empty because the service is
    // not yet ready to handle stop/shutdown. Only after initialization
    // succeeds do we transition to Running with the full control set.
    // Reporting StartPending with a 10s wait_hint avoids the SCM killing
    // the service with error 1053 if initialization is slow.
    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::StartPending,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::from_secs(10),
            process_id: None,
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let result = service_work_loop(Arc::clone(&shutdown_flag), &status_handle);

    let exit_code = if result.is_ok() {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::Win32(1)
    };

    let _ = status_handle.set_service_status(ServiceStatus {
        service_type: SERVICE_TYPE,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::ZERO,
        process_id: None,
    });

    result
}

fn service_work_loop(
    shutdown_flag: Arc<AtomicBool>,
    status_handle: &service_control_handler::ServiceStatusHandle,
) -> std::io::Result<()> {
    let config = AgentConfig::load(None::<&std::path::Path>)
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    config.ensure_dirs().map_err(|e| std::io::Error::other(e.to_string()))?;

    let log_config = LogConfig {
        log_dir: config.log_dir.clone(),
        verbose: config.verbose,
        program_name: config.program_name.clone(),
        root_dir: config.root_dir.clone(),
        restrict_permissions: config.hardening.restrict_agent_dir_permissions,
        restrict_log_permissions: config.hardening.restrict_log_dir_permissions,
    };
    let _guard = init_logging(&log_config).map_err(|e| std::io::Error::other(e.to_string()))?;

    // Write the `.version` file, which the installer package does not ship.
    // After logging init so a failure is diagnosable. Best-effort — a write
    // failure must not stop the service.
    if let Err(e) = crate::system::version_file::write() {
        tracing::warn!(
            error = %e,
            "Failed to write agent .version file; version-dependent hooks may fail"
        );
    }

    // Initialization complete — transition from StartPending to Running and
    // accept stop/shutdown/interrogate controls.
    status_handle
        .set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::ZERO,
            process_id: None,
        })
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let shutdown = ShutdownFlag::from_arc(shutdown_flag);
    worker::run(&shutdown, &config);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn try_run_returns_not_launched_by_scm_when_run_outside_scm() {
        // When the test binary runs from a console, the service
        // dispatcher must report NotLaunchedByScm rather than blocking
        // or returning an opaque error. This is the contract the
        // _worker fallback path depends on.
        let outcome = try_run().expect("try_run must not surface 1063 as error");
        assert!(
            matches!(outcome, DispatchOutcome::NotLaunchedByScm),
            "expected NotLaunchedByScm in console context, got {outcome:?}"
        );
    }

    #[cfg(windows)]
    #[test]
    fn run_returns_clear_error_when_not_launched_by_scm() {
        // The explicit `run-as-service` command must fail loudly with
        // an actionable message when invoked from a console rather
        // than silently exiting or hanging. Tested via the outcome
        // mapping: the service dispatcher itself may only be invoked
        // once per process, and the `try_run` test owns that call.
        let err = outcome_to_result(DispatchOutcome::NotLaunchedByScm)
            .expect_err("NotLaunchedByScm must map to an error");
        let msg = err.to_string();
        assert!(
            msg.contains("Service Control Manager"),
            "error message should mention SCM: {msg}"
        );
    }
}
