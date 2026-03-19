//! @risk high
//!
//! Daemon lifecycle management: PID files, signals, master-worker process model.

pub mod master;
pub mod pid_file;
pub mod signal;
pub mod worker;

pub use master::Master;
pub use pid_file::PidFile;

/// Default paths for the agent daemon.
pub const DEFAULT_PID_DIR: &str = "/opt/codedeploy-agent/state/.pid";
pub const DEFAULT_PID_FILE: &str = "master.pid";

/// Default timeout for graceful shutdown (seconds).
pub const DEFAULT_KILL_WAIT_SECS: u64 = 7200;

/// Delay before respawning a crashed worker (seconds).
pub const WORKER_RESPAWN_DELAY_SECS: u64 = 5;

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

#[cfg(not(unix))]
pub(crate) fn is_process_alive(_pid: u32) -> bool {
    false
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

#[cfg(not(unix))]
pub(crate) fn send_sigterm(_pid: u32) -> std::io::Result<()> {
    Err(std::io::Error::other("signals not supported on this platform"))
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
    fn send_sigterm_rejects_overflow_pid() {
        let err = send_sigterm(u32::MAX).unwrap_err();
        assert!(err.to_string().contains("out of valid i32 range"));
    }
}
