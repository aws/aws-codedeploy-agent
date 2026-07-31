//! Signal handling for graceful shutdown.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tracing::info;

/// Shared flag that is set when a shutdown signal is received.
#[derive(Debug, Clone)]
pub struct ShutdownFlag {
    flag: Arc<AtomicBool>,
}

impl ShutdownFlag {
    #[must_use]
    pub fn new() -> Self {
        Self { flag: Arc::new(AtomicBool::new(false)) }
    }

    /// Create a `ShutdownFlag` from an existing `Arc<AtomicBool>`.
    ///
    /// Used by the Windows service handler which owns the raw atomic flag
    /// and needs to wrap it for the worker polling loop.
    #[cfg(windows)]
    #[must_use]
    pub fn from_arc(flag: Arc<AtomicBool>) -> Self {
        Self { flag }
    }

    /// Returns `true` if shutdown has been requested.
    #[must_use]
    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Set the shutdown flag.
    pub fn set(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }
}

impl Default for ShutdownFlag {
    fn default() -> Self {
        Self::new()
    }
}

/// Register SIGTERM and SIGINT handlers that set the shutdown flag.
///
/// # Errors
/// Returns an error if signal handler registration fails.
#[cfg(unix)]
pub fn register_shutdown_handlers(flag: &ShutdownFlag) -> std::io::Result<()> {
    // signal-hook requires the flag to be a plain Arc<AtomicBool>
    let raw = Arc::clone(&flag.flag);
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&raw))?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, raw)?;
    info!("Registered SIGTERM/SIGINT shutdown handlers");
    Ok(())
}

/// Register a Windows console control handler that sets the shutdown flag
/// on Ctrl+C, Ctrl+Break, console close, logoff, or system shutdown.
///
/// # Errors
/// Returns an error if `SetConsoleCtrlHandler` fails.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn register_shutdown_handlers(flag: &ShutdownFlag) -> std::io::Result<()> {
    use std::sync::OnceLock;
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;

    /// Process-wide storage for the shutdown flag, needed because the console
    /// control handler callback is a plain `extern "system" fn` with no closure state.
    static SHUTDOWN_FLAG: OnceLock<Arc<AtomicBool>> = OnceLock::new();

    // Windows console control event constants.
    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;
    const CTRL_CLOSE_EVENT: u32 = 2;
    const CTRL_LOGOFF_EVENT: u32 = 5;
    const CTRL_SHUTDOWN_EVENT: u32 = 6;

    unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> i32 {
        match ctrl_type {
            CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT => {
                if let Some(flag) = SHUTDOWN_FLAG.get() {
                    flag.store(true, Ordering::SeqCst);
                }
                1 // TRUE — handled
            },
            _ => 0, // FALSE — let default handler run
        }
    }

    // Guard against double-registration first.
    if SHUTDOWN_FLAG.get().is_some() {
        tracing::warn!("Shutdown flag already registered; ignoring subsequent registration");
        return Ok(());
    }

    // Register the handler before storing the flag so that a
    // `SetConsoleCtrlHandler` failure doesn't poison the `OnceLock`.
    // SAFETY: `ctrl_handler` has the correct signature for `SetConsoleCtrlHandler`.
    // The second argument `1` (TRUE) means "add handler".
    let ret = unsafe { SetConsoleCtrlHandler(Some(ctrl_handler), 1) };
    if ret == 0 {
        return Err(std::io::Error::last_os_error());
    }

    // If a concurrent call raced past the guard above, the `set()`
    // failure is benign: the flag is already present.
    let _ = SHUTDOWN_FLAG.set(Arc::clone(&flag.flag));

    info!("Registered Windows console control handler");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_flag_initially_false() {
        let flag = ShutdownFlag::new();
        assert!(!flag.is_set());
    }

    #[test]
    fn shutdown_flag_set() {
        let flag = ShutdownFlag::new();
        flag.set();
        assert!(flag.is_set());
    }

    #[test]
    fn shutdown_flag_clone_shares_state() {
        let flag = ShutdownFlag::new();
        let clone = flag.clone();
        flag.set();
        assert!(clone.is_set());
    }

    #[test]
    fn shutdown_flag_default() {
        let flag = ShutdownFlag::default();
        assert!(!flag.is_set());
    }

    #[cfg(unix)]
    #[test]
    fn register_handlers_succeeds() {
        let flag = ShutdownFlag::new();
        assert!(register_shutdown_handlers(&flag).is_ok());
    }
}
