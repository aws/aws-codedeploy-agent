//! Security test helpers — assertion utilities for permission, mode,
//! and path containment checks.
//!
//! These helpers are shared across all security test modules to avoid
//! duplicating validation logic. Each helper performs a specific security
//! assertion and produces a clear failure message identifying the violation.
//!
//! Some helpers will not be used until later CRs add tests that call them.
#![allow(dead_code)]

use std::path::Path;

// ---------------------------------------------------------------------------
// Path containment assertions
// ---------------------------------------------------------------------------

/// Assert that `file` is contained within `root` after canonicalization.
///
/// If either path cannot be canonicalized (e.g., does not exist), falls back
/// to lexical comparison. This catches path-traversal attacks where an
/// extracted file escapes the deployment directory.
pub fn assert_path_contained(file: &Path, root: &Path) {
    let canonical_file = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    assert!(
        canonical_file.starts_with(&canonical_root),
        "Path {} escapes root {}",
        canonical_file.display(),
        canonical_root.display()
    );
}

/// Assert that no file under `dir` has escaped the directory via symlinks.
///
/// Walks the directory recursively and checks that every entry's canonical
/// path is still within `dir`. Detects symlinks pointing outside the tree.
#[cfg(unix)]
pub fn assert_no_symlink_escapes(dir: &Path) {
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        let path = entry.path();
        if path.is_symlink() {
            let target = std::fs::read_link(path).expect("should read symlink target");
            let resolved = if target.is_absolute() {
                target.clone()
            } else {
                path.parent().unwrap_or(Path::new(".")).join(&target)
            };
            let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            let canonical_target = resolved.canonicalize().unwrap_or(resolved);
            assert!(
                canonical_target.starts_with(&canonical_dir),
                "Symlink {} -> {} escapes root {}",
                path.display(),
                target.display(),
                canonical_dir.display()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// File permission assertions
// ---------------------------------------------------------------------------

/// Assert that no file under `dir` has SUID or SGID bits set.
///
/// Walks the directory recursively and checks the permission mode of every
/// regular file. SUID (0o4000) and SGID (0o2000) bits should never appear
/// in deployment-extracted files.
#[cfg(unix)]
pub fn assert_no_suid_sgid(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    for entry in walkdir::WalkDir::new(dir).into_iter().filter_map(Result::ok) {
        if !entry.file_type().is_file() {
            continue;
        }
        let mode = entry.metadata().expect("read entry metadata").permissions().mode();
        assert_eq!(
            mode & 0o6000,
            0,
            "SUID/SGID bits set on {}: mode={:04o}",
            entry.path().display(),
            mode
        );
    }
}

/// Assert a file has the expected Unix permissions mask.
///
/// Compares only the lower 12 bits (0o7777) of the mode, ignoring the
/// file-type bits.
#[cfg(unix)]
pub fn assert_file_mode(path: &Path, expected_mask: u32) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .expect("read file metadata for mode check")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(
        mode,
        expected_mask,
        "File {} has mode {:04o}, expected {:04o}",
        path.display(),
        mode,
        expected_mask
    );
}

/// Assert a file is NOT world-readable (no 0o004 bit).
#[cfg(unix)]
pub fn assert_not_world_readable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .expect("read file metadata for world-readable check")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode & 0o004, 0, "File {} is world-readable: mode={:04o}", path.display(), mode);
}

/// Assert a file is NOT group-readable (no 0o040 bit).
#[cfg(unix)]
pub fn assert_not_group_readable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .expect("read file metadata for group-readable check")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(mode & 0o040, 0, "File {} is group-readable: mode={:04o}", path.display(), mode);
}

/// Assert that group and world bits are all zero (restrictive permissions).
///
/// Used for state files, config files, and other sensitive data that should
/// only be accessible by the owning user.
#[cfg(unix)]
pub fn assert_owner_only_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(path)
        .expect("read file metadata for owner-only check")
        .permissions()
        .mode()
        & 0o7777;
    assert_eq!(
        mode & 0o077,
        0,
        "File {} has group/world permissions: mode={:04o} (expected owner-only)",
        path.display(),
        mode
    );
}

// ---------------------------------------------------------------------------
// Content / string assertions
// ---------------------------------------------------------------------------

/// Assert that a string does not contain raw ANSI escape sequences.
///
/// ANSI escapes start with `\x1b` (ESC). Their presence in log files can
/// enable terminal injection attacks (clear screen, fake output, title
/// manipulation).
pub fn assert_no_ansi_escapes(content: &str, context: &str) {
    assert!(
        !content.contains('\x1b'),
        "Content contains raw ANSI escape sequence ({context}): {:?}",
        &content[..content.len().min(200)]
    );
}

/// Assert that a log entry line has the expected structure: timestamp prefix + content.
///
/// Security property: script output must be clearly demarcated so that injected
/// fake log entries (e.g., `"2026-01-01 INFO Fake"`) are distinguishable from
/// real agent log lines.
pub fn assert_log_entry_prefixed(entry: &str, expected_prefix: &str) {
    assert!(
        entry.contains(expected_prefix),
        "Log entry missing prefix '{expected_prefix}': {entry}"
    );
}
