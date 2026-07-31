//! Cross-platform process operations: signals, process group management.

/// Terminate a child process and its descendants (graceful: SIGTERM only).
///
/// - Unix: sends SIGTERM to the process group (negative PID).
/// - Windows: calls `TerminateProcess` on the process handle (no process groups).
///
/// A process that ignores SIGTERM survives this; to guarantee termination,
/// follow up with [`force_kill_process_group`] after a grace period.
#[cfg_attr(windows, allow(unsafe_code))]
pub fn kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    if pid == 0 {
        tracing::warn!("Refusing to signal process group 0 (would kill own group)");
        return;
    }

    #[cfg(unix)]
    {
        let Ok(p) = i32::try_from(pid) else {
            tracing::warn!(pid, "PID exceeds i32::MAX, cannot signal process group");
            return;
        };
        let neg_pid = p.wrapping_neg();
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(neg_pid),
            nix::sys::signal::Signal::SIGTERM,
        );
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };

        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if !handle.is_null() {
            // TerminateProcess returns 0 on failure (e.g. access denied).
            if unsafe { TerminateProcess(handle, 1) } == 0 {
                tracing::warn!(pid, "TerminateProcess failed");
            }
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        }
    }
}

/// Forcibly kill a child process and its descendants (SIGKILL — uncatchable).
///
/// - Unix: sends SIGKILL to the process group (negative PID), which cannot be
///   trapped or ignored, so a SIGTERM-ignoring hook is still reaped.
/// - Windows: `TerminateProcess` is already forcible, so this mirrors
///   [`kill_process_group`].
#[cfg_attr(windows, allow(unsafe_code))]
pub fn force_kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    if pid == 0 {
        tracing::warn!("Refusing to SIGKILL process group 0 (would kill own group)");
        return;
    }

    #[cfg(unix)]
    {
        let Ok(p) = i32::try_from(pid) else {
            tracing::warn!(pid, "PID exceeds i32::MAX, cannot signal process group");
            return;
        };
        let neg_pid = p.wrapping_neg();
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(neg_pid),
            nix::sys::signal::Signal::SIGKILL,
        );
    }

    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };

        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if !handle.is_null() {
            // TerminateProcess returns 0 on failure (e.g. access denied).
            if unsafe { TerminateProcess(handle, 1) } == 0 {
                tracing::warn!(pid, "TerminateProcess failed");
            }
            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kill_none_is_noop() {
        kill_process_group(None);
    }

    #[test]
    fn kill_invalid_pid_is_noop() {
        kill_process_group(Some(999_999_999));
    }

    #[test]
    fn kill_pid_zero_is_refused() {
        // pid 0 would signal the agent's own process group (HIGH-3).
        kill_process_group(Some(0));
        force_kill_process_group(Some(0));
    }

    #[test]
    fn force_kill_none_and_invalid_are_noop() {
        force_kill_process_group(None);
        force_kill_process_group(Some(999_999_999));
    }

    #[test]
    fn kill_pid_above_i32_max_is_noop() {
        // Above i32::MAX the checked conversion must bail rather than wrap.
        kill_process_group(Some(u32::MAX));
        force_kill_process_group(Some(u32::MAX));
    }
}
