//! @risk low
//!
//! Cross-platform process operations: signals, process group management.

/// Terminate a child process and its descendants.
///
/// - Unix: sends SIGTERM to the process group (negative PID).
/// - Windows: calls `TerminateProcess` on the process handle (no process groups).
pub fn kill_process_group(pid: Option<u32>) {
    let Some(pid) = pid else { return };

    #[cfg(unix)]
    {
        #[allow(clippy::cast_possible_wrap)]
        let _ = nix::sys::signal::kill(
            nix::unistd::Pid::from_raw(-(pid as i32)),
            nix::sys::signal::Signal::SIGTERM,
        );
    }

    #[cfg(windows)]
    {
        use std::os::windows::io::FromRawHandle;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, PROCESS_TERMINATE, TerminateProcess,
        };

        let handle = unsafe { OpenProcess(PROCESS_TERMINATE, 0, pid) };
        if !handle.is_null() {
            unsafe { TerminateProcess(handle, 1) };
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
}
