//! @risk medium
//!
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

#[cfg(not(unix))]
pub fn register_shutdown_handlers(_flag: &ShutdownFlag) -> std::io::Result<()> {
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
