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

/// Build a `Mode` from `u32` permission bits. `mode_t` is `u32` on Linux but
/// `u16` on macOS, so mask to the permission bits (`0o7777`) before the cast.
#[cfg(unix)]
#[must_use]
#[allow(clippy::unnecessary_cast, clippy::cast_possible_truncation)] // width of mode_t is platform-specific
pub fn mode_from_bits(bits: u32) -> nix::sys::stat::Mode {
    nix::sys::stat::Mode::from_bits_truncate((bits & 0o7777) as nix::libc::mode_t)
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
        crate::system::secure_files::write_file_secure(path, content.as_bytes(), 0o600)
    }
}

/// System-specific file operations (platform-dependent)
#[cfg(target_os = "windows")]
pub type SystemFileOperations = WindowsFileOperations;

#[cfg(not(target_os = "windows"))]
pub type SystemFileOperations = LinuxFileOperations;

/// Whether `component` is safe to use as a single path component.
///
/// `Path::join` treats `..` as an ordinary parent component and an absolute path
/// as a full replacement, so any externally supplied string used as a directory
/// name has to be checked before it is joined. This is an allowlist of shape
/// rather than a blocklist of characters: a component must be non-empty, must not
/// be a relative-path marker, and must contain no separator or NUL byte.
///
/// Deliberately permissive about the rest, because callers pass identifiers of
/// several shapes (UUID deployment-group ids, `d-`-prefixed deployment ids). Use
/// a stricter check where the exact format is known.
#[must_use]
pub fn is_safe_path_component(component: &str) -> bool {
    !component.is_empty()
        && component != "."
        && component != ".."
        && !component.contains('\0')
        && !component.chars().any(std::path::is_separator)
        // `is_separator` is platform-specific; reject the Windows separator
        // everywhere so a spec cannot behave differently per platform.
        && !component.contains('\\')
}

/// Whether a customer-supplied, revision-relative path stays inside the directory it is joined to.
///
/// Unlike [`is_safe_path_component`] this permits separators: an `AppSpec` may legitimately be nested,
/// as in `configs/appspec.yml`. It rejects anything that could climb out of the join or re-root it --
/// a `..` component, a leading `/`, or a Windows drive prefix.
///
/// Checked lexically rather than by canonicalising, because the path is validated before the file it
/// names is known to exist, and the not-found case is an expected outcome rather than an error.
#[must_use]
pub fn is_safe_relative_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\0')
        // `is_separator` is platform-specific; reject the Windows separator everywhere so a spec
        // cannot resolve differently per platform, matching is_safe_path_component.
        && !path.contains('\\')
        && !Path::new(path).components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
}

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
            std::fs::copy(entry.path(), &entry_dest)?;
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

/// Write all of `buf` to `file` at `offset`, so several threads can write
/// different regions of one handle. Uses `pwrite` on Unix and `seek_write` on
/// Windows; `seek_write` also moves the cursor, which is harmless because every
/// write passes its own offset.
///
/// # Errors
/// Returns an error if the write fails or writes zero bytes.
pub fn write_all_at(file: &std::fs::File, buf: &[u8], offset: u64) -> io::Result<()> {
    let mut buf = buf;
    let mut offset = offset;
    while !buf.is_empty() {
        #[cfg(unix)]
        let written = std::os::unix::fs::FileExt::write_at(file, buf, offset);
        #[cfg(windows)]
        let written = std::os::windows::fs::FileExt::seek_write(file, buf, offset);

        match written {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "positioned write wrote zero bytes",
                ));
            },
            Ok(n) => {
                buf = &buf[n..];
                offset += n as u64;
            },
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {},
            Err(e) => return Err(e),
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
mod safe_relative_path_tests {
    use super::is_safe_relative_path;

    #[test]
    fn accepts_a_plain_or_nested_appspec_path() {
        assert!(is_safe_relative_path("appspec.yml"));
        assert!(is_safe_relative_path("configs/appspec.yml"));
        assert!(is_safe_relative_path("./appspec.yml"), "a CurDir component stays inside");
    }

    #[test]
    fn rejects_climbing_out_or_re_rooting() {
        for bad in [
            "../appspec.yml",
            "../../etc/passwd",
            "configs/../../appspec.yml",
            "/etc/shadow",
            "",
            "app\0spec.yml",
        ] {
            assert!(!is_safe_relative_path(bad), "must reject {bad:?}");
        }
    }

    /// Rejected on every platform, so a spec cannot resolve one way on Linux and another on Windows.
    #[test]
    fn rejects_windows_separators_and_prefixes_everywhere() {
        assert!(!is_safe_relative_path(r"..\appspec.yml"));
        assert!(!is_safe_relative_path(r"configs\appspec.yml"));
        assert!(!is_safe_relative_path(r"C:\Windows\system.ini"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn mode_from_bits_keeps_permission_bits_and_drops_file_type() {
        let mode = mode_from_bits(0o100_644);
        assert_eq!(mode, nix::sys::stat::Mode::from_bits_truncate(0o644));
        assert_eq!(mode_from_bits(0o4755), nix::sys::stat::Mode::from_bits_truncate(0o4755));
    }
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
    fn safe_path_components_are_accepted() {
        for good in [
            "dg-1",
            "d-A1B2C3D4E",
            "f47ac10b-58cc-4372-a567-0e02b2c3d479",
            "arn_like-name.with.dots",
            "..hidden",
            "a..b",
        ] {
            assert!(is_safe_path_component(good), "should accept {good:?}");
        }
    }

    #[test]
    fn unsafe_path_components_are_rejected() {
        for bad in [
            "",
            ".",
            "..",
            "../evil",
            "../../../../tmp/evil",
            "a/b",
            "/absolute",
            "trailing/",
            "back\\slash",
            "nul\0byte",
        ] {
            assert!(!is_safe_path_component(bad), "should reject {bad:?}");
        }
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

    #[test]
    fn write_all_at_writes_at_offset() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("out.bin");
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(8).unwrap();
        write_all_at(&file, b"BBBB", 4).unwrap();
        write_all_at(&file, b"AAAA", 0).unwrap();
        drop(file);
        assert_eq!(std::fs::read(&path).unwrap(), b"AAAABBBB");
    }

    #[test]
    fn write_all_at_extends_file_and_zero_fills_gap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sparse.bin");
        let file = std::fs::File::create(&path).unwrap();
        // Writing past the end extends the file; the skipped region reads as zeros.
        write_all_at(&file, b"Z", 3).unwrap();
        drop(file);
        assert_eq!(std::fs::read(&path).unwrap(), b"\0\0\0Z");
    }

    #[test]
    fn write_all_at_empty_buffer_is_noop() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("empty.bin");
        let file = std::fs::File::create(&path).unwrap();
        write_all_at(&file, b"", 0).unwrap();
        drop(file);
        assert_eq!(std::fs::read(&path).unwrap().len(), 0);
    }

    #[test]
    fn write_all_at_read_only_handle_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ro.bin");
        std::fs::write(&path, b"data").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        assert!(write_all_at(&file, b"x", 0).is_err());
    }

    #[test]
    fn write_all_at_concurrent_ranges_do_not_interleave() {
        use std::sync::Arc;
        // Threads writing distinct regions of one handle produce the exact file.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("parallel.bin");
        let chunk = 4096;
        let threads = 8;
        let file = Arc::new(std::fs::File::create(&path).unwrap());
        file.set_len((chunk * threads) as u64).unwrap();

        let handles: Vec<_> = (0..threads)
            .map(|i| {
                let file = Arc::clone(&file);
                std::thread::spawn(move || {
                    let buf = vec![b'a' + u8::try_from(i).unwrap(); chunk];
                    write_all_at(&file, &buf, (i * chunk) as u64).unwrap();
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        drop(file);

        let written = std::fs::read(&path).unwrap();
        assert_eq!(written.len(), chunk * threads);
        for i in 0..threads {
            let expected = vec![b'a' + u8::try_from(i).unwrap(); chunk];
            assert_eq!(&written[i * chunk..(i + 1) * chunk], &expected[..], "region {i} corrupted");
        }
    }
}
