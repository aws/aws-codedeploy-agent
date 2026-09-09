//! Symlink-safe destination guards for permission commands.
//!
//! The install instruction list runs every `CopyCommand` first and only then
//! the `ChangeOwner`/`ChangeMode`/`ChangeAcl`/`ChangeContext` commands. While a
//! deployment uses the documented `permissions: owner: <service-user>` pattern,
//! the destination directory is owned and writable by that non-root service
//! user for the whole Install phase. A local actor running as that account can
//! win the race between the copy and the permission op, replace a freshly
//! copied file with a symlink to an arbitrary path (e.g. `/etc/passwd`), and —
//! because the path-based `chown`/`chmod`/`setfacl`/`semanage` calls follow
//! symlinks — make the root agent operate on the link target instead. That is a
//! TOCTOU symlink-following local privilege escalation (CWE-59 / CWE-367).
//!
//! These helpers close it by refusing to follow a symlink at the destination and
//! pairing that with no-follow operations at each sink
//! (`fchownat(AT_SYMLINK_NOFOLLOW)`, an `O_NOFOLLOW` open + `fchmod`,
//! `setfacl --physical`, or — for the external `semanage`/`restorecon` path — a
//! device+inode swap check) so the check and the act cannot be split by a racing
//! swap.

#[cfg(unix)]
use std::path::Path;

/// Reject a destination that is a symbolic link.
///
/// `lstat`s the path (without following) and returns
/// [`crate::installer::InstallerError::SymlinkDestinationRejected`] when it is a symlink. A
/// non-existent path is not an error here — the individual command surfaces its
/// own "not found" failure when it operates on the path.
///
/// Fails **closed**: any `lstat` error other than `NotFound` (e.g. `EACCES`,
/// or `ELOOP` on an intermediate component) is propagated rather than silently
/// allowing the operation through, so an unexpected stat failure cannot bypass
/// the symlink guard.
///
/// # Errors
/// Returns an error if the path is a symbolic link, or if `lstat` fails for any
/// reason other than the path not existing.
#[cfg(unix)]
pub fn reject_symlink_dest(object: &Path) -> crate::installer::Result<()> {
    use crate::installer::InstallerError;
    match std::fs::symlink_metadata(object) {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(InstallerError::SymlinkDestinationRejected { object: object.to_path_buf() })
        },
        // Regular file/dir — let the command proceed.
        Ok(_) => Ok(()),
        // Does not exist yet — the individual command surfaces its own
        // "not found" failure when it operates on the path.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        // Any other stat failure (EACCES, ELOOP, …) fails closed.
        Err(e) => Err(e.into()),
    }
}

/// Resolve `object` to a canonical path while proving no symlink swap happened.
///
/// For sinks that must hand a path to an external command which itself follows
/// symlinks (`semanage`/`restorecon`), [`reject_symlink_dest`] alone leaves a
/// TOCTOU window: a local actor can race a symlink into the path after the
/// check but before `canonicalize`, and `canonicalize` would then resolve it.
///
/// This captures the `lstat` device+inode of `object` *before* canonicalizing,
/// then re-`lstat`s the canonicalized result and verifies it is (a) not a
/// symlink and (b) the same underlying inode. If a swap raced in between, the
/// identities differ (or the pre-image is a symlink) and we reject. There is no
/// no-follow variant of `semanage`, so this device+inode equality check is the
/// second-layer defense the path-based sinks (`fchownat`/`O_NOFOLLOW`) get for
/// free.
///
/// # Errors
/// Returns [`crate::installer::InstallerError::SymlinkDestinationRejected`] if `object` is a
/// symlink or its identity changed across canonicalization, or an IO error if
/// the path cannot be `lstat`ed/canonicalized.
#[cfg(unix)]
pub fn canonicalize_no_symlink_swap(
    object: &std::path::Path,
) -> crate::installer::Result<std::path::PathBuf> {
    use crate::installer::InstallerError;
    use std::os::unix::fs::MetadataExt;

    let reject = || InstallerError::SymlinkDestinationRejected { object: object.to_path_buf() };

    // Pre-image: must exist and must not itself be a symlink.
    let before = std::fs::symlink_metadata(object)?;
    if before.file_type().is_symlink() {
        return Err(reject());
    }

    let canonical = std::fs::canonicalize(object)?;

    // Post-image: re-lstat the canonical path. It must not be a symlink and must
    // be the same inode on the same device as the pre-image. A racing swap
    // (the pre-image file replaced by a symlink, or the path now resolving
    // elsewhere) breaks one of these invariants.
    let after = std::fs::symlink_metadata(&canonical)?;
    if after.file_type().is_symlink() || before.dev() != after.dev() || before.ino() != after.ino()
    {
        return Err(reject());
    }

    Ok(canonical)
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;
    use crate::installer::InstallerError;
    use std::fs;

    #[test]
    fn regular_file_is_allowed() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("real.txt");
        fs::write(&file, "data").unwrap();
        assert!(reject_symlink_dest(&file).is_ok());
    }

    #[test]
    fn nonexistent_path_is_allowed() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope");
        assert!(reject_symlink_dest(&missing).is_ok());
    }

    #[test]
    fn stat_error_other_than_not_found_fails_closed() {
        // Use a regular file as if it were a directory component: lstat of
        // `<file>/child` fails with ENOTDIR, not NotFound. The guard must
        // propagate the error (fail closed), not silently allow it through.
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("a_file");
        fs::write(&file, "data").unwrap();
        let through_file = file.join("child");

        let result = reject_symlink_dest(&through_file);
        assert!(
            matches!(result, Err(InstallerError::Io(_))),
            "non-NotFound stat error must fail closed, got {result:?}"
        );
    }

    #[test]
    fn symlink_is_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "data").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = reject_symlink_dest(&link).unwrap_err();
        assert!(matches!(err, InstallerError::SymlinkDestinationRejected { .. }));
    }

    #[test]
    fn dangling_symlink_is_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        let link = dir.path().join("dangling");
        std::os::unix::fs::symlink("/nonexistent/target", &link).unwrap();

        let err = reject_symlink_dest(&link).unwrap_err();
        assert!(matches!(err, InstallerError::SymlinkDestinationRejected { .. }));
    }

    #[test]
    fn canonicalize_no_swap_accepts_regular_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let file = dir.path().join("real.txt");
        fs::write(&file, "data").unwrap();

        let resolved = canonicalize_no_symlink_swap(&file).expect("regular file must resolve");
        assert_eq!(resolved, std::fs::canonicalize(&file).unwrap());
    }

    #[test]
    fn canonicalize_no_swap_rejects_symlink_preimage() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "data").unwrap();
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let err = canonicalize_no_symlink_swap(&link).unwrap_err();
        assert!(matches!(err, InstallerError::SymlinkDestinationRejected { .. }));
    }

    #[test]
    fn canonicalize_no_swap_errors_on_missing_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope");
        // Unlike reject_symlink_dest, the canonicalizing variant requires the
        // path to exist (semanage needs a concrete target), so a missing path
        // is an IO error rather than Ok.
        assert!(canonicalize_no_symlink_swap(&missing).is_err());
    }
}
