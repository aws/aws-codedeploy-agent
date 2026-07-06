//! @risk medium
//!
//! Linux-specific system operations — `setfacl`.
use std::io;
use std::path::Path;
#[cfg(not(coverage))]
use std::process::Command;

/// Trait for Linux file permission operations (setfacl, chmod, chown)
pub trait LinuxOps {
    /// Set POSIX ACL using setfacl.
    ///
    /// `physical` selects `setfacl --physical` (operate on a symlink itself,
    /// no-follow) vs. the default symlink-following behavior. See
    /// [`crate::installer::commands::ChangeAclCommand`].
    /// # Errors
    /// Returns an error if setting ACL fails.
    fn set_acl(&self, acl_string: &str, path: &Path, physical: bool) -> io::Result<()>;
}

/// System implementation that executes real setfacl/chmod/chown commands
#[derive(Debug, Clone, Copy)]
pub struct SystemLinuxOps;

// System command implementation excluded from coverage builds because it
// shells out to setfacl binary. Business logic tested via MockLinuxOps.
#[cfg(not(coverage))]
impl LinuxOps for SystemLinuxOps {
    fn set_acl(&self, acl_string: &str, path: &Path, physical: bool) -> io::Result<()> {
        // SECURITY: acl_string comes from AppSpec file (user input) but is safe because
        // Command::args() passes arguments directly to the binary without shell interpolation.
        // This is more secure than shell-based approaches which are
        // vulnerable to shell injection. Malformed ACL strings will be rejected by setfacl itself.
        let path_str = path.to_string_lossy();
        // By default this runs `setfacl --set` with no `--physical`/`-P`, so it
        // follows symlinks (backwards-compatible behavior).
        // SECURITY (CWE-59/CWE-367): under the opt-in
        // `reject_symlink_permission_targets`, `physical` is set so `--physical`
        // makes setfacl operate on a symlink itself rather than following it to
        // the target — defense-in-depth behind the lstat-reject in
        // ChangeAclCommand::execute.
        let mut cmd = Command::new("setfacl");
        if physical {
            cmd.arg("--physical");
        }
        let output = cmd.args(["--set", acl_string, path_str.as_ref()]).output()?;

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
    fn set_acl(&self, _acl_string: &str, _path: &Path, _physical: bool) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
#[derive(Debug, Default)]
pub struct MockLinuxOps {
    pub should_fail: bool,
    /// Captures the last `acl_string` passed to `set_acl`, so tests can assert
    /// on the exact synthesized `setfacl --set` argument.
    pub last_acl: std::cell::RefCell<Option<String>>,
}

#[cfg(test)]
impl MockLinuxOps {
    #[must_use]
    pub fn with_failure() -> Self {
        Self { should_fail: true, last_acl: std::cell::RefCell::new(None) }
    }
}

#[cfg(test)]
impl LinuxOps for MockLinuxOps {
    /// # Errors
    /// Returns an error if setting ACL fails.
    fn set_acl(&self, acl_string: &str, _path: &Path, _physical: bool) -> io::Result<()> {
        *self.last_acl.borrow_mut() = Some(acl_string.to_string());
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
        let mock = MockLinuxOps::default();
        assert!(mock.set_acl("u:user:rwx", Path::new("/tmp/test"), true).is_ok());
    }

    #[test]
    fn mock_linux_ops_failure() {
        let mock = MockLinuxOps::with_failure();
        assert!(mock.set_acl("u:user:rwx", Path::new("/tmp/test"), false).is_err());
    }
}
