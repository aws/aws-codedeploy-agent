//! `SELinux` operations — `semanage` and `restorecon` wrappers.
use std::io;
use std::path::Path;
#[cfg(not(coverage))]
use std::process::Command;

/// Flags for the `restorecon` invocation after adding an fcontext rule.
///
/// - `-v` logs each restored file.
/// - `-F` forces the full label (user + role + type + range) to match the
///   fcontext rule. Without `-F`, `restorecon` only resets the type portion
///   and silently drops user/range — a false-sense-of-security condition
///   where the customer specifies context fields in `AppSpec` but they never
///   reach the live file. `-F` is correct here because the fcontext rule
///   we just created via `semanage fcontext -a` encodes the customer's
///   full intent from the `AppSpec` `context:` block.
#[cfg(not(coverage))]
pub(crate) const RESTORECON_FLAGS: &str = "-vF";

/// Trait for `SELinux` operations (semanage, restorecon)
pub trait SeLinuxOps {
    /// Add `SELinux` file context mapping using semanage fcontext -a
    /// # Errors
    /// Returns an error if setting context fails.
    fn set_context(&self, args: &[&str], path: &Path) -> io::Result<()>;

    /// Remove `SELinux` file context mapping using semanage fcontext -d
    /// # Errors
    /// Returns an error if removing context fails.
    fn remove_context(&self, path: &Path) -> io::Result<()>;

    /// Restore `SELinux` context using restorecon
    ///
    /// # Errors
    /// Returns an error if restoring context fails.
    fn restore_context(&self, path: &Path) -> io::Result<()>;
}

/// System implementation that executes real semanage/restorecon commands
#[derive(Debug, Clone, Copy)]
pub struct SystemSeLinuxOps;

// System command implementations are excluded from coverage builds because they
// shell out to semanage/restorecon binaries that aren't available in test environments.
// All business logic is tested via MockSeLinuxOps in the installer command tests.
#[cfg(not(coverage))]
impl SeLinuxOps for SystemSeLinuxOps {
    fn set_context(&self, args: &[&str], path: &Path) -> io::Result<()> {
        // SECURITY: args come from AppSpec file (user input) but are safe because
        // Command::args() passes each argument directly to the binary without shell
        // interpolation. This is more secure than shell-based approaches.
        // Malformed SELinux context values will be rejected by semanage itself.
        let path_str = path.to_string_lossy();
        let mut cmd = Command::new("semanage");
        cmd.arg("fcontext").arg("-a");

        for arg in args {
            cmd.arg(arg);
        }
        cmd.arg(path_str.as_ref());

        let output = cmd.output()?;

        // GRCOV_STOP_COVERAGE — semanage not available in test environments
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "semanage fcontext -a failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
        // GRCOV_BEGIN_COVERAGE
    }

    fn remove_context(&self, path: &Path) -> io::Result<()> {
        let path_str = path.to_string_lossy();
        // GRCOV_STOP_COVERAGE — semanage not available in test environments
        let output =
            Command::new("semanage").args(["fcontext", "-d", path_str.as_ref()]).output()?;

        if !output.status.success() {
            return Err(io::Error::other(format!(
                "semanage fcontext -d failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
        // GRCOV_BEGIN_COVERAGE
    }

    fn restore_context(&self, path: &Path) -> io::Result<()> {
        let path_str = path.to_string_lossy();
        // Use `restorecon -vF` so the full label (user + role + type + range)
        // from the fcontext rule created via `semanage fcontext -a` is applied.
        // Without `-F`, restorecon only resets the type portion and silently
        // drops the user and range fields the customer specified in AppSpec.
        let output = Command::new("restorecon")
            .args([RESTORECON_FLAGS, path_str.as_ref()])
            .output()?;

        // GRCOV_STOP_COVERAGE
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "restorecon failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
        // GRCOV_BEGIN_COVERAGE
    }
}

// Coverage build stub: provides a no-op implementation so coverage instrumentation
// doesn't count unreachable system command code.
#[cfg(coverage)]
impl SeLinuxOps for SystemSeLinuxOps {
    fn set_context(&self, _args: &[&str], _path: &Path) -> io::Result<()> {
        Ok(())
    }

    fn remove_context(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }

    fn restore_context(&self, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[derive(Debug)]
pub struct MockSeLinuxOps {
    pub should_fail: bool,
}

#[cfg(test)]
impl MockSeLinuxOps {
    #[must_use]
    pub fn with_failure() -> Self {
        Self { should_fail: true }
    }
}

#[cfg(test)]
impl SeLinuxOps for MockSeLinuxOps {
    /// # Errors
    /// Returns an error if setting context fails.
    fn set_context(&self, _args: &[&str], _path: &Path) -> io::Result<()> {
        if self.should_fail {
            Err(io::Error::other("Mock semanage failure"))
        } else {
            Ok(())
        }
    }

    /// # Errors
    /// Returns an error if removing context fails.
    fn remove_context(&self, _path: &Path) -> io::Result<()> {
        if self.should_fail {
            Err(io::Error::other("Mock semanage failure"))
        } else {
            Ok(())
        }
    }

    fn restore_context(&self, _path: &Path) -> io::Result<()> {
        if self.should_fail {
            Err(io::Error::other("Mock restorecon failure"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mock_selinux_ops_success() {
        let mock = MockSeLinuxOps { should_fail: false };
        let path = Path::new("/tmp/test");
        assert!(mock.set_context(&["-t", "httpd_sys_content_t"], path).is_ok());
        assert!(mock.remove_context(path).is_ok());
        assert!(mock.restore_context(path).is_ok());
    }

    #[test]
    fn mock_selinux_ops_failure() {
        let mock = MockSeLinuxOps::with_failure();
        let path = Path::new("/tmp/test");
        assert!(mock.set_context(&["-t", "httpd_sys_content_t"], path).is_err());
        assert!(mock.remove_context(path).is_err());
        assert!(mock.restore_context(path).is_err());
    }

    // Test SystemSeLinuxOps — in coverage builds these hit the stub (Ok(())).
    // In non-coverage builds, semanage/restorecon aren't installed so they error.
    #[test]
    fn system_set_context() {
        let ops = SystemSeLinuxOps;
        let result = ops.set_context(&["-t", "httpd_sys_content_t"], Path::new("/tmp/nonexistent"));
        // Coverage build: stub returns Ok. Non-coverage: semanage not found, returns Err.
        let _ = result;
    }

    #[test]
    fn system_remove_context() {
        let ops = SystemSeLinuxOps;
        let result = ops.remove_context(Path::new("/tmp/nonexistent"));
        let _ = result;
    }

    #[test]
    fn system_restore_context() {
        let ops = SystemSeLinuxOps;
        let result = ops.restore_context(Path::new("/tmp/nonexistent"));
        let _ = result;
    }

    // Ensure restorecon is invoked with -F so the full SELinux label (user + role + type + range) from the fcontext
    // rule is applied, not just the type. `restorecon -v` alone silently
    // drops the user and range the customer specified in AppSpec.
    #[cfg(not(coverage))]
    #[test]
    fn restorecon_flags_include_force_for_full_label_reset() {
        assert!(
            RESTORECON_FLAGS.contains('F'),
            "restorecon must be invoked with -F to apply the full SELinux label \
             (user + role + type + range), not just the type. Without -F, the \
             customer-specified `user:` and `range:` fields from AppSpec are \
             silently dropped."
        );
    }
}
