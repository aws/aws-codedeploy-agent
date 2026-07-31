//! Daemon lifecycle management: PID files, signals, master-worker process model.

pub mod core_dumps;
pub mod master;
pub mod pid_file;
pub mod signal;
#[cfg(windows)]
pub mod windows_service;
pub mod worker;

pub use master::Master;
pub use pid_file::PidFile;

/// Default paths for the agent daemon.
///
/// Returns the platform-appropriate PID directory via [`crate::paths::pid_dir`].
#[must_use]
pub fn default_pid_dir() -> std::path::PathBuf {
    crate::paths::pid_dir()
}
/// PID file name (`<program_name>.pid`), so operators and tooling that look for
/// `codedeploy-agent.pid` under the PID dir keep working. No separate flock lock
/// file is created — single-instance is enforced by the service manager plus PID
/// liveness.
pub const DEFAULT_PID_FILE: &str = "codedeploy-agent.pid";

/// Default timeout for graceful shutdown (seconds).
pub const DEFAULT_KILL_WAIT_SECS: u64 = 7200;

/// Initial delay before respawning a crashed worker (seconds).
pub const WORKER_RESPAWN_DELAY_SECS: u64 = 5;

/// Upper bound for the exponential backoff between respawns (seconds).
pub const WORKER_RESPAWN_MAX_DELAY_SECS: u64 = 60;

/// Minimum uptime after which a worker is considered healthy and the respawn
/// backoff is reset to [`WORKER_RESPAWN_DELAY_SECS`].
pub const WORKER_HEALTHY_UPTIME_SECS: u64 = 60;

/// Check if a process is alive by sending signal 0.
///
/// # Safety of cast
/// Linux PIDs are limited to `pid_max` (default 32768, max 4194304 on 64-bit),
/// well within `i32` range. The clippy allow is safe.
///
/// # EPERM edge case
/// Returns `true` for EPERM (process exists but owned by another user).
/// POSIX semantics: EPERM means the process exists but the caller lacks
/// permission to signal it. In practice the agent runs as root so EPERM
/// is not expected.
#[cfg(unix)]
pub(crate) fn is_process_alive(pid: u32) -> bool {
    let Ok(pid_i32) = i32::try_from(pid) else {
        return false; // PID out of valid i32 range
    };
    match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid_i32), None) {
        Ok(()) | Err(nix::errno::Errno::EPERM) => true, // process exists (EPERM = no permission)
        Err(_) => false,
    }
}

/// Check if a process is alive on Windows.
///
/// Opens the process with `SYNCHRONIZE` and calls `WaitForSingleObject`
/// with a zero timeout to check whether it is still running.
///
/// Returns `true` for `ERROR_ACCESS_DENIED` (process exists but we lack
/// rights), analogous to the Unix EPERM edge case.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn is_process_alive(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ACCESS_DENIED, GetLastError, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{OpenProcess, WaitForSingleObject};

    const SYNCHRONIZE: u32 = 0x0010_0000;
    // SAFETY: OpenProcess with SYNCHRONIZE is a read-only operation.
    // A null return means the process doesn't exist or we lack access.
    let handle = unsafe { OpenProcess(SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        // SAFETY: GetLastError is safe immediately after a failed Win32 call.
        return unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
    }
    // SAFETY: handle is valid (non-null) and timeout=0 makes this non-blocking.
    let alive = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    // SAFETY: handle is valid and we own it exclusively.
    unsafe { CloseHandle(handle) };
    alive
}

/// Send SIGTERM to a process.
///
/// See [`is_process_alive`] for cast safety rationale.
#[cfg(unix)]
pub(crate) fn send_sigterm(pid: u32) -> std::io::Result<()> {
    let pid_i32 = i32::try_from(pid)
        .map_err(|_| std::io::Error::other(format!("PID {pid} out of valid i32 range")))?;
    nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid_i32), nix::sys::signal::Signal::SIGTERM)
        .map_err(|e| std::io::Error::other(e.to_string()))
}

/// Terminate a process on Windows (hard kill).
///
/// Windows has no SIGTERM equivalent. This calls `TerminateProcess`, which is
/// the Win32 equivalent of `SIGKILL`.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn send_sigterm(pid: u32) -> std::io::Result<()> {
    use std::io;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_TERMINATE, TerminateProcess};

    // SAFETY: OpenProcess with PROCESS_TERMINATE is a standard Win32 call.
    let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: handle is valid (non-null). Exit code 1 signals abnormal termination.
    let ret = unsafe { TerminateProcess(handle, 1) };
    let result = if ret == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    };
    // SAFETY: handle is valid and we own it. Runs on both success and failure
    // paths to prevent resource leaks.
    unsafe { CloseHandle(handle) };
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_process_alive_current() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    fn is_process_alive_dead() {
        assert!(!is_process_alive(99_999_999));
    }

    #[test]
    fn is_process_alive_returns_false_for_overflow_pid() {
        assert!(!is_process_alive(u32::MAX));
    }

    #[test]
    fn send_sigterm_to_nonexistent_process_fails() {
        assert!(send_sigterm(99_999_999).is_err());
    }

    #[test]
    #[cfg(unix)]
    fn send_sigterm_rejects_overflow_pid() {
        let err = send_sigterm(u32::MAX).unwrap_err();
        assert!(err.to_string().contains("out of valid i32 range"));
    }

    #[test]
    #[cfg(windows)]
    fn windows_is_process_alive_current_process() {
        assert!(is_process_alive(std::process::id()));
    }

    #[test]
    #[cfg(windows)]
    fn windows_is_process_alive_dead_pid() {
        assert!(!is_process_alive(99_999_999));
    }

    #[test]
    #[cfg(windows)]
    fn windows_send_sigterm_to_nonexistent_fails() {
        assert!(send_sigterm(99_999_999).is_err());
    }
}
