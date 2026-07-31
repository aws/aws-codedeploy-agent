//! Core-dump suppression for the agent process.

/// Disable core dumps for the current process. Best-effort — failures
/// are logged via `eprintln!` (runs before tracing init).
#[cfg(unix)]
pub fn disable() {
    use nix::sys::resource::{Resource, setrlimit};

    match setrlimit(Resource::RLIMIT_CORE, 0, 0) {
        Ok(()) => {},
        // GRCOV_STOP_COVERAGE
        Err(e) => {
            eprintln!("WARNING: Failed to set RLIMIT_CORE=0; core dumps may be produced: {e}");
        },
        // GRCOV_BEGIN_COVERAGE
    }

    #[cfg(target_os = "linux")]
    {
        use nix::sys::prctl;
        match prctl::set_dumpable(false) {
            Ok(()) => {},
            Err(e) => eprintln!("WARNING: Failed to set PR_SET_DUMPABLE=0: {e}"), // GRCOV_IGNORE_LINE
        }
    }
}

/// Suppress Windows Error Reporting crash dialogs and dump generation.
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn disable() {
    use windows_sys::Win32::System::Diagnostics::Debug::{
        SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode,
    };
    // SAFETY: SetErrorMode has no preconditions and only flips process-wide flags.
    let _prev = unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use nix::sys::resource::{Resource, getrlimit};

    #[test]
    fn disable_sets_rlimit_core_to_zero() {
        disable();
        let (soft, hard) = getrlimit(Resource::RLIMIT_CORE).expect("getrlimit");
        assert_eq!(soft, 0, "soft RLIMIT_CORE must be 0 after disable()");
        assert_eq!(hard, 0, "hard RLIMIT_CORE must be 0 after disable()");
    }

    #[test]
    fn disable_is_idempotent() {
        disable();
        disable();
        disable();
        let (soft, hard) = getrlimit(Resource::RLIMIT_CORE).expect("getrlimit");
        assert_eq!(soft, 0);
        assert_eq!(hard, 0);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn disable_sets_pr_set_dumpable_false() {
        use nix::sys::prctl;
        disable();
        let dumpable = prctl::get_dumpable().expect("get_dumpable");
        assert!(!dumpable, "process must not be dumpable after disable()");
    }
}
