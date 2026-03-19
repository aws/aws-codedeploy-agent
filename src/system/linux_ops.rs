//! @risk medium
//!
//! Linux-specific system operations — `setfacl`.
use std::io;
use std::path::Path;
#[cfg(not(coverage))]
use std::process::Command;

/// Trait for Linux file permission operations (setfacl, chmod, chown)
pub trait LinuxOps {
    /// Set POSIX ACL using setfacl
    /// # Errors
    /// Returns an error if setting ACL fails.
    fn set_acl(&self, acl_string: &str, path: &Path) -> io::Result<()>;
}

/// System implementation that executes real setfacl/chmod/chown commands
#[derive(Debug, Clone, Copy)]
pub struct SystemLinuxOps;

// System command implementation excluded from coverage builds because it
// shells out to setfacl binary. Business logic tested via MockLinuxOps.
#[cfg(not(coverage))]
impl LinuxOps for SystemLinuxOps {
    fn set_acl(&self, acl_string: &str, path: &Path) -> io::Result<()> {
        // SECURITY: acl_string comes from AppSpec file (user input) but is safe because
        // Command::args() passes arguments directly to the binary without shell interpolation.
        // This is more secure than shell-based approaches which are
        // vulnerable to shell injection. Malformed ACL strings will be rejected by setfacl itself.
        let path_str = path.to_string_lossy();
        let output = Command::new("setfacl")
            .args(["--set", acl_string, path_str.as_ref()])
            .output()?;

        if !output.status.success() {
            return Err(io::Error::other(format!(
                "setfacl failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }
}

#[cfg(coverage)]
impl LinuxOps for SystemLinuxOps {
    fn set_acl(&self, _acl_string: &str, _path: &Path) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[derive(Debug)]
pub struct MockLinuxOps {
    pub should_fail: bool,
}

#[cfg(test)]
impl MockLinuxOps {
    #[must_use]
    pub fn with_failure() -> Self {
        Self { should_fail: true }
    }
}

#[cfg(test)]
impl LinuxOps for MockLinuxOps {
    /// # Errors
    /// Returns an error if setting ACL fails.
    fn set_acl(&self, _acl_string: &str, _path: &Path) -> io::Result<()> {
        if self.should_fail {
            Err(io::Error::other("Mock setfacl failure"))
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
    fn mock_linux_ops_success() {
        let mock = MockLinuxOps { should_fail: false };
        assert!(mock.set_acl("u:user:rwx", Path::new("/tmp/test")).is_ok());
    }

    #[test]
    fn mock_linux_ops_failure() {
        let mock = MockLinuxOps::with_failure();
        assert!(mock.set_acl("u:user:rwx", Path::new("/tmp/test")).is_err());
    }
}
