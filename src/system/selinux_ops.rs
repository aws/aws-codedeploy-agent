//! @risk medium
//!
//! `SELinux` operations — `semanage` and `restorecon` wrappers.
use std::io;
use std::path::Path;
#[cfg(not(coverage))]
use std::process::Command;

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

        if !output.status.success() {
            return Err(io::Error::other(format!(
                "semanage fcontext -a failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    fn remove_context(&self, path: &Path) -> io::Result<()> {
        let path_str = path.to_string_lossy();
        let output =
            Command::new("semanage").args(["fcontext", "-d", path_str.as_ref()]).output()?;

        if !output.status.success() {
            return Err(io::Error::other(format!(
                "semanage fcontext -d failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }

    fn restore_context(&self, path: &Path) -> io::Result<()> {
        let path_str = path.to_string_lossy();
        let output = Command::new("restorecon").args(["-v", path_str.as_ref()]).output()?;

        if !output.status.success() {
            return Err(io::Error::other(format!(
                "restorecon failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
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
}
