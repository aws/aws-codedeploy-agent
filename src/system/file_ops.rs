//! @risk medium
//!
//! Platform-specific file operations with retry logic
//!
//! This module provides a trait-based abstraction for file operations that need
//! platform-specific behavior. On Windows, file writes are retried with exponential
//! backoff to handle intermittent permission errors. On Linux, operations are direct.

use std::io;
use std::path::Path;
#[cfg(not(coverage))]
use std::time::Duration;

/// Trait for platform-specific file operations
pub trait PlatformFileOperations: Send + Sync {
    /// Write content to file with retry logic for Windows file locking issues
    ///
    /// # Errors
    /// Returns an error if all retry attempts fail
    fn write_with_retry(&self, path: &Path, content: &str) -> io::Result<()>;
}

/// If the file has no execute bits, add them. Mirrors
/// `script_executable?` check + `make_executable` fallback.
///
/// # Errors
/// Returns an error if metadata cannot be read or permissions cannot be set.
pub fn ensure_executable(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(path).map_err(|e| e.to_string())?;
        let perms = metadata.permissions();
        if perms.mode() & 0o111 == 0 {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(perms.mode() | 0o111))
                .map_err(|e| format!("not_executable: {e}"))?;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Linux implementation - no retry needed
///
/// `restrict` mirrors `restrict_agent_dir_permissions`: tracker state files
/// are written 0644 by default (world-readable, backwards-compatible) and
/// 0600 under opt-in hardening.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxFileOperations {
    pub restrict: bool,
}

impl LinuxFileOperations {
    #[must_use]
    pub fn with_policy(restrict: bool) -> Self {
        Self { restrict }
    }
}

impl PlatformFileOperations for LinuxFileOperations {
    fn write_with_retry(&self, path: &Path, content: &str) -> io::Result<()> {
        crate::system::secure_files::write_file_secure(
            path,
            content.as_bytes(),
            crate::system::agent_file_mode(self.restrict),
        )
    }
}

/// Windows implementation - retries on EACCES errors
///
/// `restrict` is accepted for interface parity but has no effect on Windows:
/// `write_file_secure` always applies the SYSTEM+Administrators DACL there.
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsFileOperations {
    pub restrict: bool,
}

impl WindowsFileOperations {
    #[must_use]
    pub fn with_policy(restrict: bool) -> Self {
        Self { restrict }
    }
}

// Windows retry logic excluded from coverage on Linux builds where PermissionDenied
// retry behavior cannot be meaningfully tested. Tested on Windows CI.
#[cfg(not(coverage))]
impl PlatformFileOperations for WindowsFileOperations {
    fn write_with_retry(&self, path: &Path, content: &str) -> io::Result<()> {
        const RETRY_DELAYS_MS: [u64; 3] = [1000, 2000, 5000];

        for (attempt, &delay_ms) in RETRY_DELAYS_MS.iter().enumerate() {
            // `mode` is ignored on Windows; `write_file_secure` applies the
            // SYSTEM+Administrators DACL atomically via CreateFileW.
            match crate::system::secure_files::write_file_secure(path, content.as_bytes(), 0o600) {
                Ok(()) => return Ok(()),
                // GRCOV_STOP_COVERAGE — Windows retry logic, untestable on Linux
                Err(e)
                    if e.kind() == io::ErrorKind::PermissionDenied
                        && attempt < RETRY_DELAYS_MS.len() - 1 =>
                {
                    std::thread::sleep(Duration::from_millis(delay_ms));
                },
                Err(e) => return Err(e),
            }
        }
        Ok(())
        // GRCOV_BEGIN_COVERAGE
    }
}

#[cfg(coverage)]
impl PlatformFileOperations for WindowsFileOperations {
    fn write_with_retry(&self, path: &Path, content: &str) -> io::Result<()> {
        crate::system::secure_files::write_file_secure(path, content.as_bytes(), 0o600)
    }
}

/// System-specific file operations (platform-dependent)
#[cfg(target_os = "windows")]
pub type SystemFileOperations = WindowsFileOperations;

#[cfg(not(target_os = "windows"))]
pub type SystemFileOperations = LinuxFileOperations;

/// Recursively copy a directory tree.
///
/// # Errors
/// Returns an error if any file or directory operation fails.
pub fn copy_dir_recursive(src: &Path, dest: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let entry_dest = dest.join(entry.file_name());
        let file_type = entry.file_type()?;

        if file_type.is_symlink() {
            #[cfg(unix)]
            {
                let target = std::fs::read_link(entry.path())?;
                std::os::unix::fs::symlink(&target, &entry_dest)?;
            }
            #[cfg(not(unix))]
            std::fs::copy(entry.path(), &entry_dest)?; // GRCOV_IGNORE_LINE
        } else if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &entry_dest)?;
        } else {
            std::fs::copy(entry.path(), &entry_dest)?;
        }
    }
    // Preserve the source dir's mode (create_dir_all applies umask instead,
    // dropping e.g. a bundle's 0755 scripts/ to 0750 and breaking runas: hooks).
    // Set last, after children are written in.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(src)?.permissions().mode();
        std::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(test)]
#[derive(Debug)]
pub struct MockFileOperations {
    pub should_fail: bool,
}

#[cfg(test)]
impl PlatformFileOperations for MockFileOperations {
    fn write_with_retry(&self, _path: &Path, _content: &str) -> io::Result<()> {
        if self.should_fail {
            Err(io::Error::new(io::ErrorKind::PermissionDenied, "mock error"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn linux_write_with_retry() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        let ops = LinuxFileOperations::default();

        ops.write_with_retry(&path, "content").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "content");
    }

    #[cfg(unix)]
    #[test]
    fn linux_write_with_retry_default_applies_0644_mode() {
        // Tracking files are world-readable by default so host tooling
        // outside the agent can keep reading them.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("d-tracking-perms");
        let ops = LinuxFileOperations::default();

        ops.write_with_retry(&path, "cmd-456").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "tracking file mode {mode:#o}, want 0644 (world-readable)");
    }

    #[cfg(unix)]
    #[test]
    fn linux_write_with_retry_restricted_applies_0600_mode() {
        // Opt-in hardening: tracking files hold deployment ID + command ID
        // used by crash recovery — owner-only.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("d-tracking-perms");
        let ops = LinuxFileOperations::with_policy(true);

        ops.write_with_retry(&path, "cmd-456").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "tracking file mode {mode:#o}, want 0600");
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial(umask)]
    fn linux_write_with_retry_ignores_umask() {
        // Mode is applied via fchmod, not umask, so a permissive umask
        // can't widen the result.
        use nix::sys::stat::{Mode, umask};
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("d-tracking-umask");
        let ops = LinuxFileOperations::default();

        let prev = umask(Mode::from_bits_truncate(0o000));
        let result = ops.write_with_retry(&path, "cmd-456");
        umask(prev);
        result.unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "tracking file mode {mode:#o}, want 0644 despite umask 0000");
    }

    #[test]
    fn windows_write_with_retry_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        let ops = WindowsFileOperations::default();

        ops.write_with_retry(&path, "content").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "content");
    }

    #[test]
    fn mock_write_success() {
        let mock = MockFileOperations { should_fail: false };
        let path = PathBuf::from("/fake/path");

        assert!(mock.write_with_retry(&path, "content").is_ok());
    }

    #[test]
    fn mock_write_failure() {
        let mock = MockFileOperations { should_fail: true };
        let path = PathBuf::from("/fake/path");

        assert!(mock.write_with_retry(&path, "content").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn copy_dir_recursive_with_symlink() {
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        std::fs::create_dir_all(&src).unwrap();

        // Create a regular file and a symlink to it
        std::fs::write(src.join("file.txt"), "hello").unwrap();
        std::os::unix::fs::symlink("file.txt", src.join("link.txt")).unwrap();

        copy_dir_recursive(&src, &dest).unwrap();

        assert!(dest.join("file.txt").exists());
        assert!(dest.join("link.txt").is_symlink());
        assert_eq!(
            std::fs::read_link(dest.join("link.txt")).unwrap().to_str().unwrap(),
            "file.txt"
        );
    }

    #[test]
    #[cfg(unix)]
    fn copy_dir_recursive_preserves_dir_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("src");
        let dest = dir.path().join("dest");
        // 0751: not reachable from `mkdir(0777) & ~umask` under any common
        // umask (0022->0755, 0027->0750), so this fails if mode preservation
        // is removed regardless of the test runner's umask.
        std::fs::create_dir_all(src.join("scripts")).unwrap();
        std::fs::set_permissions(src.join("scripts"), std::fs::Permissions::from_mode(0o751))
            .unwrap();
        std::fs::write(src.join("scripts/h.sh"), "#!/bin/sh\n").unwrap();

        copy_dir_recursive(&src, &dest).unwrap();

        let mode = std::fs::metadata(dest.join("scripts")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o751, "expected 0751 preserved, got {mode:#o}");
    }
}
