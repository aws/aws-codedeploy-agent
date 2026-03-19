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
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxFileOperations;

impl PlatformFileOperations for LinuxFileOperations {
    fn write_with_retry(&self, path: &Path, content: &str) -> io::Result<()> {
        std::fs::write(path, content)
    }
}

/// Windows implementation - retries on EACCES errors
#[derive(Debug, Clone, Copy, Default)]
pub struct WindowsFileOperations;

// Windows retry logic excluded from coverage on Linux builds where PermissionDenied
// retry behavior cannot be meaningfully tested. Tested on Windows CI.
#[cfg(not(coverage))]
impl PlatformFileOperations for WindowsFileOperations {
    fn write_with_retry(&self, path: &Path, content: &str) -> io::Result<()> {
        const RETRY_DELAYS_MS: [u64; 3] = [1000, 2000, 5000];

        for (attempt, &delay_ms) in RETRY_DELAYS_MS.iter().enumerate() {
            match std::fs::write(path, content) {
                Ok(()) => return Ok(()),
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
    }
}

#[cfg(coverage)]
impl PlatformFileOperations for WindowsFileOperations {
    fn write_with_retry(&self, path: &Path, content: &str) -> io::Result<()> {
        std::fs::write(path, content)
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
            let target = std::fs::read_link(entry.path())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &entry_dest)?;
            #[cfg(not(unix))]
            std::fs::copy(entry.path(), &entry_dest)?;
        } else if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &entry_dest)?;
        } else {
            std::fs::copy(entry.path(), &entry_dest)?;
        }
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
        let ops = LinuxFileOperations;

        ops.write_with_retry(&path, "content").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "content");
    }

    #[test]
    fn windows_write_with_retry_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test.txt");
        let ops = WindowsFileOperations;

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
}
