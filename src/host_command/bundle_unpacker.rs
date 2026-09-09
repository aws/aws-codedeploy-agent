//! Bundle archive unpacking.
//!
//! Extracts tar/tgz/zip archives and strips a leading directory if the archive
//! contains a single top-level folder with an appspec file.
//!
//! Uses system commands (tar, unzip) with a native Rust fallback for both when
//! the system binary is unavailable or fails.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use tracing::debug;

/// Unpack a bundle archive into `dest`, then strip any leading directory.
///
/// `restrict_permissions` selects the archive-dir mode policy: `false`
/// (default, backwards-compatible) gives world-readable 0755; `true` (opt-in
/// hardening via `restrict_agent_dir_permissions`) gives 0711 —
/// traversable for `runas:` hook execution but not listable.
///
/// `ignore_ownership` selects the extracted-file ownership policy for
/// tar/tgz bundles: `false` (default, backwards-compatible) lets root tar
/// apply the archive's stored uid/gid (`--same-owner`, GNU tar's root
/// default); `true` (opt-in hardening via `ignore_ownership_in_bundle`)
/// forces files to be owned by the extracting process. Zip bundles are
/// root-owned under both policies (`unzip` never applies archive ownership
/// without `-X`).
///
/// # Errors
/// Returns an error if extraction or directory stripping fails.
pub fn unpack(
    bundle_path: &Path,
    dest: &Path,
    bundle_type: &str,
    restrict_permissions: bool,
    ignore_ownership: bool,
) -> io::Result<()> {
    debug!(
        bundle_type,
        bundle = %bundle_path.display(),
        dest = %dest.display(),
        "Unpacking bundle archive"
    );

    let dest_mode = archive_dir_mode(restrict_permissions);
    crate::system::create_deployment_dir(dest, 0o711, restrict_permissions)?;

    match bundle_type {
        "tgz" => unpack_tgz(bundle_path, dest, ignore_ownership)?,
        "zip" => unpack_zip(bundle_path, dest)?,
        // "tar" and anything else default to tar
        _ => unpack_tar(bundle_path, dest, ignore_ownership)?,
    }

    // GNU tar running as root applies stored permissions, including the
    // archive's `.` entry — which can lower `dest` from the policy mode to
    // whatever the bundle's top-level dir mode was (commonly 0700 from
    // `mktemp -d`). Restore it so `runas:` traversal keeps working.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dest, fs::Permissions::from_mode(dest_mode))?;
    }
    #[cfg(not(unix))]
    let _ = dest_mode;

    strip_leading_directory(dest, restrict_permissions)
}

/// Archive-dir mode for the given policy: 0755 by default (backwards
/// compatible), 0711 (traversable, not listable) under opt-in hardening.
pub(crate) fn archive_dir_mode(restrict_permissions: bool) -> u32 {
    if restrict_permissions { 0o711 } else { 0o755 }
}

fn unpack_tar(bundle: &Path, dest: &Path, ignore_ownership: bool) -> io::Result<()> {
    unpack_tar_with_system(bundle, dest, false, ignore_ownership)
}

fn unpack_tgz(bundle: &Path, dest: &Path, ignore_ownership: bool) -> io::Result<()> {
    unpack_tar_with_system(bundle, dest, true, ignore_ownership)
}

/// Extract a tar (optionally gzipped) bundle, preferring the system `tar`
/// binary and falling back to native Rust extraction when it is unavailable or
/// fails. Mirrors [`unpack_zip`].
fn unpack_tar_with_system(
    bundle: &Path,
    dest: &Path,
    gzipped: bool,
    ignore_ownership: bool,
) -> io::Result<()> {
    // System tar behavior on a 0-byte file differs: GNU tar rejects it,
    // while libarchive-based bsdtar (the system tar on Windows) accepts it
    // as a valid empty archive and exits 0. Reject it up front so an
    // empty/corrupt bundle fails uniformly on every platform, matching
    // the check in `unpack_tar_native`.
    if std::fs::metadata(bundle)?.len() == 0 {
        return Err(io::Error::other("archive is empty (0 bytes); not a valid tar archive"));
    }

    let extract_flag = if gzipped { "-xzf" } else { "-xf" };
    // SECURITY: root tar defaults to --same-owner, chowning every extracted
    // file to the bundle-builder's (usually non-root) header uid — leaving
    // hooks/appspec writable to a local uid that collides with it. Under the
    // opt-in `ignore_ownership_in_bundle` flag, --no-same-owner forces root
    // ownership; the default (no flag) keeps the historical extraction
    // behavior — root tar applies header ownership, non-root tar ignores it.
    // We do NOT add --no-same-permissions: ownership (not mode) is the
    // writable-ness lever, and stripping modes would hide SUID/SGID from the
    // opt-in reject_unsafe_permissions_in_bundle scan.
    let mut args = vec![extract_flag.to_string(), bundle.display().to_string()];
    if ignore_ownership {
        args.push("--no-same-owner".to_string());
    }
    args.push("-C".to_string());
    args.push(dest.display().to_string());
    match Command::new("tar").args(&args).output() {
        Ok(output) if output.status.success() => return Ok(()),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(
                exit_code = output.status.code().unwrap_or(-1),
                "System tar failed, falling back to native tar extraction: {stderr}"
            );
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            debug!("System tar binary not found, using native tar extraction");
        },
        Err(e) => {
            debug!(
                "System tar binary failed to launch ({e}), falling back to native tar extraction"
            );
        },
    }

    unpack_tar_native(bundle, dest, gzipped, ignore_ownership)
}

/// Native tar fallback for when the system `tar` binary is unavailable.
/// Mirrors [`unpack_zip_native`].
///
/// Extracts faithfully (symlinks, hardlinks, and permission bits preserved) to
/// match system `tar`; rejection is left to the flag-gated post-extraction
/// scans the caller runs. `Entry::unpack_in` guarantees no write escapes `dest`.
fn unpack_tar_native(
    bundle: &Path,
    dest: &Path,
    gzipped: bool,
    ignore_ownership: bool,
) -> io::Result<()> {
    let file = fs::File::open(bundle)?;

    // The `tar` crate accepts a 0-byte file as a valid empty archive; system
    // `tar` rejects it. Reject it here too so the fallback doesn't silently
    // accept an empty/corrupt bundle. (Non-empty garbage still errors below.)
    if file.metadata()?.len() == 0 {
        return Err(io::Error::other("archive is empty (0 bytes); not a valid tar archive"));
    }

    if gzipped {
        let decoder = flate2::read::GzDecoder::new(file);
        unpack_tar_entries(tar::Archive::new(decoder), dest, ignore_ownership)
    } else {
        unpack_tar_entries(tar::Archive::new(file), dest, ignore_ownership)
    }
}

fn unpack_tar_entries<R: io::Read>(
    mut archive: tar::Archive<R>,
    dest: &Path,
    ignore_ownership: bool,
) -> io::Result<()> {
    // Match system `tar`: preserve stored mode bits (consistent with the
    // post-extraction unsafe-permission scan and the 0711 dest reset) and
    // overwrite existing files.
    archive.set_preserve_permissions(true);
    // SECURITY: with `ignore_ownership_in_bundle` set, never apply the
    // archive's stored uid/gid — mirrors `--no-same-owner` on the system-tar
    // path. Default (flag off) mirrors GNU tar's root-conditional
    // `--same-owner`, the historical extraction behavior: apply header
    // ownership only when extracting as root; non-root extraction
    // (deploy-local, tests) ignores it, as GNU tar does, since chown would
    // fail with EPERM.
    #[cfg(unix)]
    let apply_header_ownership = !ignore_ownership && nix::unistd::Uid::effective().is_root();
    #[cfg(not(unix))]
    let apply_header_ownership = {
        let _ = ignore_ownership;
        false
    };
    archive.set_preserve_ownerships(apply_header_ownership);
    archive.set_overwrite(true);

    for entry in archive.entries()? {
        let mut entry = entry?;

        // A `..`/absolute entry can't resolve inside `dest`; skip it rather than
        // abort (matching system `tar` and the native zip path). When the flag
        // is set the pre-extraction check already rejected such bundles.
        if first_unsafe_component(&entry.path()?).is_some() {
            continue;
        }

        entry.unpack_in(dest)?;
    }

    Ok(())
}

/// Argument list for the system `unzip` invocation.
///
/// OWNERSHIP: must never include `-X` — that is the flag that makes `unzip`
/// restore uid/gid from the zip's Unix extra fields. Without it, extracted
/// files are owned by the extracting process (root) under BOTH ownership
/// policies. Zip is deliberately not gated by `ignore_ownership_in_bundle`;
/// adding `-X` here would silently put zip bundles on the same
/// bundle-controlled-ownership attack path that flag exists to close. Pinned
/// by `unzip_args_never_restore_ownership`.
fn unzip_args(bundle: &Path, dest: &Path) -> Vec<String> {
    vec![
        "-o".to_string(),
        bundle.display().to_string(),
        "-d".to_string(),
        dest.display().to_string(),
    ]
}

fn unpack_zip(bundle: &Path, dest: &Path) -> io::Result<()> {
    match Command::new("unzip").args(unzip_args(bundle, dest)).output() {
        Ok(output) if output.status.success() => return Ok(()),
        Ok(output) if output.status.code() == Some(50) => {
            let _ = fs::remove_dir_all(dest);
            return Err(io::Error::other("The disk is (or was) full during extraction."));
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            debug!(
                exit_code = output.status.code().unwrap_or(-1),
                "System unzip failed, falling back to native zip extraction: {stderr}"
            );
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            debug!("System unzip not found, using native zip extraction");
        },
        Err(e) => {
            debug!("System unzip failed to launch ({e}), falling back to native zip extraction");
        },
    }

    unpack_zip_native(bundle, dest)
}

/// Native zip fallback for when the system `unzip` binary is unavailable.
/// Mirrors [`unpack_tar_native`].
///
/// Extracts faithfully (real symlinks, permission bits preserved) to match
/// system `unzip`; rejection is left to the flag-gated post-extraction scans.
/// `..`/absolute entries are skipped, not fatal — matching system `unzip`/`tar`,
/// which strip the leading `../` rather than aborting.
///
/// Hand-rolled rather than the crate's `ZipArchive::extract` because that aborts
/// the whole archive on a traversal entry. The per-entry loop instead guards
/// each write: the parent dir is canonicalized and confirmed inside `dest`,
/// closing the symlink-write-through window (e.g. `link -> /tmp` then
/// `link/payload`) before the post-scans run.
fn unpack_zip_native(bundle: &Path, dest: &Path) -> io::Result<()> {
    use std::io::Read;

    let dest_canon = fs::canonicalize(dest)?;
    let file = fs::File::open(bundle)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| io::Error::other(format!("failed to read zip archive: {e}")))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| io::Error::other(format!("failed to read zip entry {i}: {e}")))?;

        // Skip `..`/absolute entries (see fn doc).
        let Some(enclosed) = entry.enclosed_name() else {
            continue;
        };
        let out_path = dest.join(enclosed);

        if entry.is_dir() {
            create_dir_inside(&out_path, &dest_canon)?;
            continue;
        }

        // Create the parent only if it stays inside `dest`; skip rather than
        // write through (or create dirs through) a symlink that escapes.
        if let Some(parent) = out_path.parent()
            && !create_dir_inside(parent, &dest_canon)?
        {
            continue;
        }

        // Guard the leaf: a prior entry may have created a symlink at this exact
        // path (two entries that normalize to the same `out_path`, e.g. raw
        // names "payload" and "./payload" — distinct names so the zip crate does
        // not dedup them). `File::create`/`symlink` would then follow or collide
        // with that link, letting a regular-file entry write THROUGH it to an
        // external target. `symlink_metadata` inspects the leaf without
        // following; removing the symlink makes the write land inside `dest`.
        // Dangling symlinks are removed too (no canonicalize needed).
        if let Ok(meta) = out_path.symlink_metadata()
            && meta.file_type().is_symlink()
        {
            fs::remove_file(&out_path)?;
        }

        if entry.is_symlink() {
            // A symlink entry's contents are the link target.
            let mut target = Vec::new();
            entry.read_to_end(&mut target)?;
            #[cfg(unix)]
            {
                use std::os::unix::ffi::OsStringExt;
                std::os::unix::fs::symlink(std::ffi::OsString::from_vec(target), &out_path)?;
            }
            // Non-unix has no symlinks; write the target as a file, as the zip
            // crate itself does.
            #[cfg(not(unix))]
            {
                fs::write(&out_path, &target)?;
            }
            continue;
        }

        let mut out_file = fs::File::create(&out_path)?;
        io::copy(&mut entry, &mut out_file)?;

        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&out_path, fs::Permissions::from_mode(mode))?;
        }
    }

    Ok(())
}

/// Create `dir` (and missing parents) only if it stays inside `dest_canon`,
/// returning `Ok(false)` if it would escape (e.g. through a symlink left by an
/// earlier entry). Checks the longest existing ancestor *before* creating any
/// directory, so no dir is created outside `dest`, then re-verifies afterward
/// to guard against a symlinked intermediate component.
fn create_dir_inside(dir: &Path, dest_canon: &Path) -> io::Result<bool> {
    let mut ancestor = dir;
    while !ancestor.exists() {
        match ancestor.parent() {
            Some(p) => ancestor = p,
            None => break,
        }
    }
    if !fs::canonicalize(ancestor)?.starts_with(dest_canon) {
        return Ok(false);
    }

    fs::create_dir_all(dir)?;
    Ok(fs::canonicalize(dir)?.starts_with(dest_canon))
}

/// If the archive has a single top-level directory containing an appspec,
/// move its contents up one level (strip the wrapper directory).
fn strip_leading_directory(dest: &Path, restrict_permissions: bool) -> io::Result<()> {
    let entries: Vec<_> = fs::read_dir(dest)?.filter_map(Result::ok).map(|e| e.path()).collect();

    if entries.len() != 1 || !entries[0].is_dir() {
        return Ok(());
    }

    let inner = &entries[0];

    // Check if inner dir contains an appspec file
    let has_appspec = fs::read_dir(inner)?
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().to_lowercase().contains("appspec"));

    if !has_appspec {
        return Ok(());
    }

    debug!("Stripping leading directory from archive bundle contents.");

    let temp = dest.with_file_name("deployment-archive-temp");
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    fs::rename(dest, &temp)?;
    fs::rename(temp.join(inner.file_name().unwrap()), dest)?;
    fs::remove_dir(&temp)?;

    // Restore the policy mode on dest — the rename replaced it with the
    // wrapper dir's mode (whatever the archive header carried, masked
    // through umask).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            dest,
            fs::Permissions::from_mode(archive_dir_mode(restrict_permissions)),
        )?;
    }
    #[cfg(not(unix))]
    let _ = restrict_permissions;

    Ok(())
}

/// Scan an extracted bundle directory for symlinks and hardlinks.
///
/// When `reject_symlinks_in_bundle` is enabled, this function walks the
/// extraction directory after `unpack()` completes. If any entry is a
/// symbolic link, the extraction directory is removed and an error is
/// returned.
///
/// Hardlinks are detected by checking if any regular file has a link count > 1.
///
/// # Errors
/// Returns an error naming the offending entry if a symlink or hardlink is found.
pub fn reject_bundle_symlinks(dest: &Path) -> io::Result<()> {
    if let Err(e) = scan_bundle_for_links(dest) {
        // bundle fails; not reproducible in CI without racing filesystem perms.
        if let Err(rm_err) = fs::remove_dir_all(dest) {
            tracing::error!(
                "Failed to remove rejected bundle directory {}: {rm_err}",
                dest.display()
            );
        }
        return Err(e);
    }
    Ok(())
}

/// Walk the extracted directory and return an error if any symlink or hardlink is found.
///
/// NOTE: Hardlink detection uses `nlink() > 1` which catches the typical threat case
/// (tar hardlink entries extracting with both endpoints inside the archive). It won't
/// detect hardlinks that tar resolved as copies on some filesystems (e.g., archives
/// created with `tar --hard-dereference` store duplicates instead of link entries),
/// and could theoretically false-positive if the extraction filesystem has pre-existing
/// hardlinks to the destination inode (very unusual for a fresh extraction dir).
///
/// NOTE: Windows hardlinks are intentionally not detected (`#[cfg(unix)]` gate).
/// Windows `nlink()` equivalent requires `GetFileInformationByHandle` — tracked
/// separately if Windows bundle-trust hardening is needed.
fn scan_bundle_for_links(dest: &Path) -> io::Result<()> {
    for entry in walkdir::WalkDir::new(dest).follow_links(false) {
        let entry = entry.map_err(|e| io::Error::other(format!("walkdir error: {e}")))?;
        let path = entry.path();

        // Use entry.file_type() — walkdir already captured it during readdir,
        // avoiding a redundant lstat syscall per entry.
        if entry.file_type().is_symlink() {
            let target = std::fs::read_link(path).unwrap_or_default();
            return Err(io::Error::other(format!(
                "Bundle rejected: archive contains symbolic link '{}' -> '{}'. \
                 Set reject_symlinks_in_bundle: false to allow symlinks.",
                path.strip_prefix(dest).unwrap_or(path).display(),
                target.display()
            )));
        }

        // Use entry.metadata() to avoid a re-stat and close the TOCTOU window
        // between is_file() and fs::metadata(path).
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata =
                entry.metadata().map_err(|e| io::Error::other(format!("metadata error: {e}")))?;
            if metadata.is_file() && metadata.nlink() > 1 {
                return Err(io::Error::other(format!(
                    "Bundle rejected: archive contains hardlink '{}' (link count: {}). \
                     Set reject_symlinks_in_bundle: false to allow hardlinks.",
                    path.strip_prefix(dest).unwrap_or(path).display(),
                    metadata.nlink()
                )));
            }
        }
    }
    Ok(())
}

/// Reject extracted bundles containing files with SUID/SGID bits. No-op on non-Unix.
///
/// # Errors
/// Returns an error naming the offending entry if a SUID/SGID file is found.
pub fn reject_bundle_unsafe_permissions(dest: &Path) -> io::Result<()> {
    if let Err(e) = scan_bundle_for_unsafe_permissions(dest) {
        // bundle fails; not reproducible in CI without racing filesystem perms.
        if let Err(rm_err) = fs::remove_dir_all(dest) {
            tracing::error!(
                "Failed to remove rejected bundle directory {}: {rm_err}",
                dest.display()
            );
        }
        return Err(e);
    }
    Ok(())
}

#[cfg(unix)]
fn scan_bundle_for_unsafe_permissions(dest: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    for entry in walkdir::WalkDir::new(dest).follow_links(false) {
        let entry = entry.map_err(|e| io::Error::other(format!("walkdir error: {e}")))?;

        if !entry.file_type().is_file() {
            continue;
        }

        let metadata =
            entry.metadata().map_err(|e| io::Error::other(format!("metadata error: {e}")))?;
        let mode = metadata.permissions().mode();
        if mode & 0o6000 != 0 {
            let path = entry.path();
            return Err(io::Error::other(format!(
                "deployment rejected: file '{}' has unsafe permission mode {:04o} \
                 (SUID/SGID bits set); set reject_unsafe_permissions_in_bundle: false to allow",
                path.strip_prefix(dest).unwrap_or(path).display(),
                mode & 0o7777
            )));
        }
    }
    Ok(())
}

#[cfg(not(unix))]
fn scan_bundle_for_unsafe_permissions(_dest: &Path) -> io::Result<()> {
    Ok(())
}

/// Pre-extraction header scan: reject archives whose entry paths contain `..` or absolute prefixes.
///
/// Trusts archive headers; pair with [`reject_bundle_path_traversal`] to defend
/// against PAX/LongLink desync between the Rust `tar` crate and `/bin/tar`.
///
/// # Errors
/// Returns an error if any entry path is unsafe or the archive cannot be read.
pub fn check_path_traversal(bundle_path: &Path, bundle_type: &str) -> io::Result<()> {
    match bundle_type {
        "tgz" => inspect_tar_paths(bundle_path, true),
        "zip" => inspect_zip_paths(bundle_path),
        _ => inspect_tar_paths(bundle_path, false),
    }
}

fn inspect_tar_paths(bundle_path: &Path, gzipped: bool) -> io::Result<()> {
    let file = fs::File::open(bundle_path)?;
    if gzipped {
        let decoder = flate2::read::GzDecoder::new(file);
        check_tar_entries(tar::Archive::new(decoder))
    } else {
        check_tar_entries(tar::Archive::new(file))
    }
}

fn check_tar_entries<R: io::Read>(mut archive: tar::Archive<R>) -> io::Result<()> {
    for entry in archive.entries()? {
        let entry = entry?;
        let path = entry.path()?;
        if let Some(bad) = first_unsafe_component(&path) {
            return Err(io::Error::other(format!(
                "Archive extraction rejected: entry '{}' contains path-traversal component '{bad}'; \
                 set reject_path_traversal_in_bundle: false to allow",
                path.display()
            )));
        }
    }
    Ok(())
}

fn inspect_zip_paths(bundle_path: &Path) -> io::Result<()> {
    let file = fs::File::open(bundle_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| io::Error::other(format!("failed to read zip archive: {e}")))?;

    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| io::Error::other(format!("failed to read zip entry {i}: {e}")))?;
        let raw_name = entry.name();
        let path = Path::new(raw_name);
        if let Some(bad) = first_unsafe_component(path) {
            return Err(io::Error::other(format!(
                "Archive extraction rejected: zip entry '{raw_name}' contains path-traversal component '{bad}'; \
                 set reject_path_traversal_in_bundle: false to allow"
            )));
        }
    }
    Ok(())
}

/// First `..` / root / Windows-prefix component, or `None` if the path is safe.
fn first_unsafe_component(path: &Path) -> Option<String> {
    use std::path::Component;
    for component in path.components() {
        match component {
            Component::ParentDir => return Some("..".to_string()),
            Component::RootDir => return Some("/".to_string()),
            // yields Component::Prefix on Linux regardless of input string.
            Component::Prefix(p) => return Some(p.as_os_str().to_string_lossy().into_owned()),
            Component::CurDir | Component::Normal(_) => {},
        }
    }
    None
}

/// Post-extraction scan: each entry's canonicalized path must stay under `dest`'s canonical path.
///
/// Catches extractor bugs and PAX/LongLink desync that the header scan can miss.
/// Symlinks are skipped (handled by [`reject_bundle_symlinks`]).
///
/// # Errors
/// Returns an error naming the offending entry and its resolved location.
pub fn reject_bundle_path_traversal(dest: &Path) -> io::Result<()> {
    if let Err(e) = scan_bundle_for_traversal(dest) {
        // bundle fails; not reproducible in CI without racing filesystem perms.
        if let Err(rm_err) = fs::remove_dir_all(dest) {
            tracing::error!(
                "Failed to remove rejected bundle directory {}: {rm_err}",
                dest.display()
            );
        }
        return Err(e);
    }
    Ok(())
}

fn scan_bundle_for_traversal(dest: &Path) -> io::Result<()> {
    let dest_canon = fs::canonicalize(dest)?;

    for entry in walkdir::WalkDir::new(dest).follow_links(false) {
        let entry = entry.map_err(|e| io::Error::other(format!("walkdir error: {e}")))?;
        let path = entry.path();

        if entry.file_type().is_symlink() {
            continue;
        }

        let resolved = match fs::canonicalize(path) {
            Ok(p) => p,
            Err(e) => {
                return Err(io::Error::other(format!(
                    "Bundle rejected: failed to canonicalize '{}': {e}",
                    path.display()
                )));
            },
        };

        if !resolved.starts_with(&dest_canon) {
            return Err(io::Error::other(format!(
                "Bundle rejected: extracted entry '{}' resolves to '{}' outside destination '{}'; \
                 set reject_path_traversal_in_bundle: false to allow",
                path.strip_prefix(dest).unwrap_or(path).display(),
                resolved.display(),
                dest_canon.display()
            )));
        }
    }
    Ok(())
}

/// Check that the archive's declared total uncompressed size does not exceed `max_size`.
///
/// Walks archive headers using native crates without extracting. For tar/tgz,
/// uses the `tar` crate; for zip, uses the `zip` crate.
///
/// Note: this is a header-level check — it trusts the size fields declared by the
/// archive. A malicious bundle that lies about its declared size can bypass this cap.
/// The cap is best paired with disk-space monitoring during extraction for full
/// bomb protection.
///
/// # Errors
/// Returns an error if the declared size exceeds `max_size` or if the archive
/// cannot be read.
pub fn check_extraction_size(
    bundle_path: &Path,
    bundle_type: &str,
    max_size: u64,
) -> io::Result<()> {
    let total_declared = match bundle_type {
        "tgz" => inspect_tar_gz(bundle_path, max_size)?,
        "zip" => inspect_zip(bundle_path, max_size)?,
        _ => inspect_tar(bundle_path, max_size)?,
    };

    if total_declared > max_size {
        return Err(io::Error::other(format!(
            "Archive extraction rejected: declared size {total_declared} bytes \
             exceeds limit {max_size} bytes"
        )));
    }

    Ok(())
}

/// Inspect a plain tar archive. Returns total declared size in bytes.
fn inspect_tar(bundle_path: &Path, max_size: u64) -> io::Result<u64> {
    let file = fs::File::open(bundle_path)?;
    inspect_tar_entries(tar::Archive::new(file), max_size)
}

/// Inspect a gzip-compressed tar archive. Returns total declared size in bytes.
fn inspect_tar_gz(bundle_path: &Path, max_size: u64) -> io::Result<u64> {
    let file = fs::File::open(bundle_path)?;
    let decoder = flate2::read::GzDecoder::new(file);
    inspect_tar_entries(tar::Archive::new(decoder), max_size)
}

/// Walk tar entries and sum declared sizes, exiting early once `max_size` is exceeded.
///
/// Returns a lower bound on total declared size. The returned value equals the true
/// total only when it is ≤ `max_size`; otherwise iteration stops early.
///
/// Truncated or malformed archives produce an error — they never pass through to
/// extraction.
fn inspect_tar_entries<R: io::Read>(
    mut archive: tar::Archive<R>,
    max_size: u64,
) -> io::Result<u64> {
    let mut total_size: u64 = 0;
    for entry in archive.entries()? {
        let entry = entry?;
        total_size = total_size.saturating_add(entry.size());
        if total_size > max_size {
            return Ok(total_size);
        }
    }
    Ok(total_size)
}

/// Inspect a zip archive natively. Returns total declared uncompressed size in bytes.
fn inspect_zip(bundle_path: &Path, max_size: u64) -> io::Result<u64> {
    let file = fs::File::open(bundle_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| io::Error::other(format!("failed to read zip archive: {e}")))?;

    let mut total_size: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .map_err(|e| io::Error::other(format!("failed to read zip entry {i}: {e}")))?;
        total_size = total_size.saturating_add(entry.size());
        if total_size > max_size {
            return Ok(total_size);
        }
    }

    Ok(total_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn strip_single_dir_with_appspec() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("deployment-archive");
        let inner = dest.join("my-app");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("appspec.yml"), "version: 0.0").unwrap();
        fs::write(inner.join("script.sh"), "#!/bin/sh").unwrap();

        strip_leading_directory(&dest, false).unwrap();

        assert!(dest.join("appspec.yml").exists());
        assert!(dest.join("script.sh").exists());
        assert!(!dest.join("my-app").exists());
    }

    #[test]
    fn strip_no_op_multiple_entries() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("deployment-archive");
        fs::create_dir_all(dest.join("dir1")).unwrap();
        fs::create_dir_all(dest.join("dir2")).unwrap();

        strip_leading_directory(&dest, false).unwrap();

        assert!(dest.join("dir1").exists());
        assert!(dest.join("dir2").exists());
    }

    #[test]
    fn strip_no_op_single_dir_without_appspec() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("deployment-archive");
        let inner = dest.join("my-app");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("readme.txt"), "hello").unwrap();

        strip_leading_directory(&dest, false).unwrap();

        // Not stripped — no appspec
        assert!(dest.join("my-app").exists());
    }

    #[test]
    fn strip_no_op_single_file() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("deployment-archive");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("appspec.yml"), "version: 0.0").unwrap();

        strip_leading_directory(&dest, false).unwrap();

        // Single file, not a dir — no stripping
        assert!(dest.join("appspec.yml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unpack_tar_roundtrip() {
        let dir = TempDir::new().unwrap();

        // Create a tar with a file
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        unpack(&tar_path, &dest, "tar", false, false).unwrap();

        assert!(dest.join("hello.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unpack_tgz_roundtrip() {
        let dir = TempDir::new().unwrap();

        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let tgz_path = dir.path().join("bundle.tgz");
        Command::new("tar")
            .args([
                "-czf",
                &tgz_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        unpack(&tgz_path, &dest, "tgz", false, false).unwrap();

        assert!(dest.join("hello.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unpack_unknown_type_defaults_to_tar() {
        let dir = TempDir::new().unwrap();

        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        unpack(&tar_path, &dest, "unknown", false, false).unwrap();

        assert!(dest.join("hello.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn strip_leading_directory_cleans_up_existing_temp() {
        let dir = TempDir::new().unwrap();

        let src = dir.path().join("wrapper");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("appspec.yml"), "version: 0.0").unwrap();
        fs::write(src.join("file.txt"), "content").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &dir.path().display().to_string(),
                "wrapper",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");

        // Create temp directory that should be cleaned up
        let temp = dest.with_file_name("deployment-archive-temp");
        fs::create_dir_all(&temp).unwrap();

        unpack(&tar_path, &dest, "tar", false, false).unwrap();

        // Verify temp was cleaned up and files were stripped
        assert!(!temp.exists());
        assert!(dest.join("appspec.yml").exists());
        assert!(dest.join("file.txt").exists());
    }

    #[cfg(unix)]
    fn make_wrapper_bundle(dir: &TempDir) -> std::path::PathBuf {
        // A tarball with a single wrapper directory containing an appspec,
        // so strip_leading_directory fires.
        let src = dir.path().join("wrapper");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("appspec.yml"), "version: 0.0").unwrap();
        fs::write(src.join("script.sh"), "#!/bin/sh").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &dir.path().display().to_string(),
                "wrapper",
            ])
            .output()
            .unwrap();
        tar_path
    }

    #[cfg(unix)]
    #[test]
    fn unpack_default_gives_world_readable_archive_dir_after_strip() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let tar_path = make_wrapper_bundle(&dir);

        let dest = dir.path().join("deployment-archive");
        unpack(&tar_path, &dest, "tar", false, false).unwrap();

        // strip_leading_directory should have fired (single wrapper with appspec)
        assert!(dest.join("appspec.yml").exists());
        assert!(!dest.join("wrapper").exists());

        // Default policy: the dest directory must be 0755 after the strip
        let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "expected 0755 but got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn unpack_restricted_preserves_0711_mode_after_strip() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let tar_path = make_wrapper_bundle(&dir);

        let dest = dir.path().join("deployment-archive");
        unpack(&tar_path, &dest, "tar", true, false).unwrap();

        assert!(dest.join("appspec.yml").exists());
        assert!(!dest.join("wrapper").exists());

        // Hardened policy: the dest directory must still be 0711 after the strip
        let mode = fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o711, "expected 0711 but got {mode:o}");
    }

    #[cfg(unix)]
    #[test]
    fn scan_detects_symlink() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dest.join("link")).unwrap();

        let err = scan_bundle_for_links(&dest).unwrap_err();
        assert!(err.to_string().contains("symbolic link"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn size_cap_rejects_oversized_tar() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("test.tar");

        let mut f = fs::File::create(&archive).unwrap();
        let header = raw_tar_header(b"big.bin", 1024 * 1024);
        f.write_all(&header).unwrap();
        f.write_all(&[0u8; 512]).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();

        let err = check_extraction_size(&archive, "tar", 512 * 1024).unwrap_err();
        assert!(err.to_string().contains("declared size"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn scan_detects_hardlink() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        let original = dest.join("original.txt");
        fs::write(&original, "data").unwrap();
        fs::hard_link(&original, dest.join("hardlink.txt")).unwrap();

        let err = scan_bundle_for_links(&dest).unwrap_err();
        assert!(err.to_string().contains("hardlink"), "got: {err}");
    }

    #[test]
    fn scan_passes_clean_directory() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("file.txt"), "content").unwrap();
        fs::create_dir_all(dest.join("subdir")).unwrap();
        fs::write(dest.join("subdir/nested.txt"), "nested").unwrap();

        assert!(scan_bundle_for_links(&dest).is_ok());
    }

    #[test]
    fn reject_bundle_symlinks_passes_clean_dir() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("file.txt"), "content").unwrap();

        assert!(reject_bundle_symlinks(&dest).is_ok());
        assert!(dest.exists(), "clean dir should remain");
    }

    #[cfg(unix)]
    #[test]
    fn reject_bundle_symlinks_removes_dir_on_symlink() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dest.join("evil")).unwrap();

        let result = reject_bundle_symlinks(&dest);
        assert!(result.is_err());
        assert!(!dest.exists(), "rejected dir should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn reject_bundle_unsafe_permissions_passes_clean_dir() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("file.txt"), "content").unwrap();

        assert!(reject_bundle_unsafe_permissions(&dest).is_ok());
        assert!(dest.exists(), "clean dir should remain");
    }

    #[cfg(unix)]
    #[test]
    fn reject_bundle_unsafe_permissions_rejects_suid_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        let suid_file = dest.join("suid_binary");
        fs::write(&suid_file, "#!/bin/sh").unwrap();
        fs::set_permissions(&suid_file, fs::Permissions::from_mode(0o4755)).unwrap();

        let result = reject_bundle_unsafe_permissions(&dest);
        assert!(result.is_err(), "SUID file must be rejected");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("suid_binary"), "error should name the file: {msg}");
        assert!(msg.contains("4755"), "error should include the offending mode: {msg}");
        assert!(!dest.exists(), "rejected bundle dir should be removed");
    }

    #[cfg(unix)]
    #[test]
    fn reject_bundle_unsafe_permissions_rejects_sgid_file() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        let sgid_file = dest.join("sgid_binary");
        fs::write(&sgid_file, "#!/bin/sh").unwrap();
        fs::set_permissions(&sgid_file, fs::Permissions::from_mode(0o2755)).unwrap();

        let result = reject_bundle_unsafe_permissions(&dest);
        assert!(result.is_err(), "SGID file must be rejected");
        assert!(!dest.exists(), "rejected bundle dir should be removed");
    }

    #[test]
    fn first_unsafe_component_detects_parent() {
        assert_eq!(first_unsafe_component(Path::new("../etc/passwd")), Some("..".to_string()));
        assert_eq!(
            first_unsafe_component(Path::new("subdir/../../etc/passwd")),
            Some("..".to_string())
        );
    }

    #[test]
    fn first_unsafe_component_detects_absolute() {
        assert!(first_unsafe_component(Path::new("/etc/passwd")).is_some());
    }

    #[test]
    fn first_unsafe_component_allows_safe_paths() {
        assert_eq!(first_unsafe_component(Path::new("./file.txt")), None);
        assert_eq!(first_unsafe_component(Path::new("subdir/file.txt")), None);
        assert_eq!(first_unsafe_component(Path::new("a/b/c/file.txt")), None);
    }

    #[cfg(unix)]
    #[test]
    fn check_path_traversal_rejects_parent_component_in_tar() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("evil.tar");

        let mut f = fs::File::create(&archive).unwrap();
        let body = b"x";
        f.write_all(&raw_tar_header(b"../../etc/passwd", body.len() as u64)).unwrap();
        f.write_all(body).unwrap();
        f.write_all(&[0u8; 511]).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();

        let err = check_path_traversal(&archive, "tar").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("traversal component"), "got: {msg}");
        assert!(msg.contains(".."), "got: {msg}");
    }

    #[cfg(unix)]
    #[test]
    fn check_path_traversal_rejects_absolute_path_in_tar() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("evil.tar");

        let mut f = fs::File::create(&archive).unwrap();
        let body = b"x";
        f.write_all(&raw_tar_header(b"/etc/passwd", body.len() as u64)).unwrap();
        f.write_all(body).unwrap();
        f.write_all(&[0u8; 511]).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();

        let err = check_path_traversal(&archive, "tar").unwrap_err();
        assert!(err.to_string().contains("traversal component"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn check_path_traversal_allows_safe_tar() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("appspec.yml"), "version: 0.0").unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let archive = dir.path().join("safe.tar");
        Command::new("tar")
            .args([
                "-cf",
                &archive.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        assert!(check_path_traversal(&archive, "tar").is_ok());
    }

    #[test]
    fn check_path_traversal_rejects_traversal_in_zip() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("evil.zip");

        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("../../etc/passwd", opts).unwrap();
        zip.write_all(b"pwned").unwrap();
        zip.finish().unwrap();

        let err = check_path_traversal(&archive, "zip").unwrap_err();
        assert!(err.to_string().contains("traversal component"), "got: {err}");
    }

    #[test]
    fn check_path_traversal_allows_safe_zip() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("safe.zip");

        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("appspec.yml", opts).unwrap();
        zip.write_all(b"version: 0.0").unwrap();
        zip.start_file("subdir/file.txt", opts).unwrap();
        zip.write_all(b"content").unwrap();
        zip.finish().unwrap();

        assert!(check_path_traversal(&archive, "zip").is_ok());
    }

    #[test]
    fn reject_bundle_path_traversal_passes_clean_dir() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("file.txt"), "content").unwrap();
        fs::create_dir_all(dest.join("subdir")).unwrap();
        fs::write(dest.join("subdir/nested.txt"), "data").unwrap();

        assert!(reject_bundle_path_traversal(&dest).is_ok());
        assert!(dest.exists(), "clean dir should remain");
    }

    #[cfg(unix)]
    #[test]
    fn reject_bundle_path_traversal_accepts_normal_file() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("ok.txt"), "ok").unwrap();
        assert!(reject_bundle_path_traversal(&dest).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn reject_bundle_path_traversal_skips_symlinks() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dest.join("link")).unwrap();
        fs::write(dest.join("real.txt"), "data").unwrap();

        assert!(reject_bundle_path_traversal(&dest).is_ok());
        assert!(dest.exists());
    }

    #[cfg(unix)]
    #[test]
    fn reject_bundle_unsafe_permissions_allows_sticky_bit() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        let sticky_file = dest.join("sticky_file");
        fs::write(&sticky_file, "data").unwrap();
        fs::set_permissions(&sticky_file, fs::Permissions::from_mode(0o1755)).unwrap();

        assert!(
            reject_bundle_unsafe_permissions(&dest).is_ok(),
            "sticky bit alone must not be rejected"
        );
    }

    #[cfg(unix)]
    #[test]
    fn reject_logs_cleanup_failure_when_dir_unremovable() {
        use std::os::unix::fs::PermissionsExt;

        struct PermGuard<'a>(&'a Path);
        impl Drop for PermGuard<'_> {
            fn drop(&mut self) {
                let _ = fs::set_permissions(self.0, fs::Permissions::from_mode(0o755));
            }
        }

        if nix::unistd::Uid::effective().is_root() {
            // Root bypasses DAC permission checks, so the denial this test
            // relies on never happens (e.g. in CI build containers).
            return;
        }

        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("bundle");
        fs::create_dir_all(&dest).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", dest.join("evil")).unwrap();

        // Guard restores permissions on both normal exit and panic
        let _guard = PermGuard(dir.path());
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();

        let result = reject_bundle_symlinks(&dest);
        assert!(result.is_err(), "should still return the symlink error");
        assert!(
            result.unwrap_err().to_string().contains("symbolic link"),
            "error should name the symlink"
        );
        // dest still exists because removal failed (parent is read-only)
        assert!(dest.exists(), "dir should remain when cleanup fails");

        // Restore permissions for TempDir cleanup
        fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_cap_allows_any_size() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let archive = dir.path().join("test.tar");
        Command::new("tar")
            .args([
                "-cf",
                &archive.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        assert!(check_extraction_size(&archive, "tar", u64::MAX).is_ok());
    }

    /// Zip corner of the bundle-ownership hardening: the system `unzip`
    /// invocation must never pass `-X`, the flag that restores uid/gid from the
    /// zip's Unix extra fields. Without it, extracted files are owned by the
    /// extracting process (root) under both ownership policies, which is why
    /// zip needs no `ignore_ownership_in_bundle` gating. An ownership assertion
    /// can't catch `-X` in non-root CI (unzip silently skips chown as
    /// non-root), so pin the arg list itself.
    #[test]
    fn unzip_args_never_restore_ownership() {
        let args = unzip_args(Path::new("/tmp/bundle.zip"), Path::new("/tmp/out"));
        assert!(
            !args.iter().any(|a| a == "-X"),
            "unzip must not be invoked with -X (restores archive uid/gid): {args:?}"
        );
    }

    /// With `ignore_ownership_in_bundle` set, extracted files must be owned by
    /// the extracting process, never by the uid/gid stored in the tar header.
    /// The header is stamped with a uid chosen to differ from the current
    /// process uid; after extraction the file must be owned by the current
    /// process uid instead — proving the header-stored ownership is not applied
    /// (`--no-same-owner` on the system path, no ownership preservation on the
    /// native fallback).
    ///
    /// The flag-off default is the historical behavior: header ownership IS
    /// applied when extracting as root (GNU tar's root-conditional
    /// `--same-owner`), and ignored when non-root — the non-root half is
    /// asserted at the end of this test; the root half requires a root test
    /// environment and rests on the root-conditional guard in
    /// `unpack_tar_entries`.
    ///
    /// Runs the native fallback directly so the assertion is deterministic
    /// regardless of which `tar` binary CI ships.
    #[cfg(unix)]
    #[test]
    fn ignore_flag_extracts_files_not_owned_by_header_uid() {
        use std::io::Write;
        use std::os::unix::fs::MetadataExt;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("hdr_uid.tar");

        // Stamp a header uid/gid that is guaranteed to differ from the current
        // process uid — otherwise the assertion below would pass trivially when
        // CI happens to run as the header's uid (e.g. 1000, the common first
        // non-root user). raw_tar_header defaults to 1000, so pick 1001 when we
        // are 1000, else 1000.
        let current = nix::unistd::Uid::current().as_raw();
        let fake_uid: u32 = if current == 1000 { 1001 } else { 1000 };

        let mut header = raw_tar_header(b"app.txt", b"hello".len() as u64);
        let uid_octal = format!("{fake_uid:07o}");
        header[108..115].copy_from_slice(uid_octal.as_bytes()); // uid
        header[116..123].copy_from_slice(uid_octal.as_bytes()); // gid
        // Recompute the header checksum (offsets 148..156) after the edit, using
        // the same scheme as raw_tar_header.
        header[148..156].copy_from_slice(b"        ");
        let cksum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        header[148..156].copy_from_slice(format!("{cksum:06o}\0 ").as_bytes());

        let body = b"hello";
        let mut f = fs::File::create(&archive).unwrap();
        f.write_all(&header).unwrap();
        f.write_all(body).unwrap();
        f.write_all(&[0u8; 512 - 5]).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();
        drop(f);

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_tar_native(&archive, &dest, false, true).unwrap();

        let extracted = dest.join("app.txt");
        assert!(extracted.exists(), "file should be extracted");
        let uid = fs::metadata(&extracted).unwrap().uid();
        assert_ne!(
            uid, fake_uid,
            "extracted file must NOT be owned by the tar header's uid {fake_uid}"
        );
        assert_eq!(
            uid, current,
            "extracted file must be owned by the extracting process (uid {current}), got {uid}"
        );

        // Default policy (flag off), non-root: header ownership is ignored —
        // GNU tar behaves the same (chown as non-root would EPERM). Only a
        // root process applies it under the default policy.
        if !nix::unistd::Uid::effective().is_root() {
            let dest2 = dir.path().join("out2");
            fs::create_dir_all(&dest2).unwrap();
            unpack_tar_native(&archive, &dest2, false, false).unwrap();
            let uid2 = fs::metadata(dest2.join("app.txt")).unwrap().uid();
            assert_eq!(
                uid2, current,
                "non-root default extraction must keep extracting-process ownership"
            );
        }
    }

    fn raw_tar_header(name: &[u8], size: u64) -> [u8; 512] {
        let mut header = [0u8; 512];
        let len = name.len().min(100);
        header[..len].copy_from_slice(&name[..len]);
        header[100..107].copy_from_slice(b"0000644");
        header[108..115].copy_from_slice(b"0001000");
        header[116..123].copy_from_slice(b"0001000");
        let size_str = format!("{size:011o}");
        header[124..135].copy_from_slice(size_str.as_bytes());
        header[136..147].copy_from_slice(b"14717450000");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let cksum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let cksum_str = format!("{cksum:06o}\0 ");
        header[148..156].copy_from_slice(cksum_str.as_bytes());
        header
    }

    #[cfg(unix)]
    #[test]
    fn size_cap_rejects_oversized_tgz() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        // Create a file larger than our cap
        fs::write(src.join("big.bin"), vec![0u8; 2048]).unwrap();

        let archive = dir.path().join("test.tgz");
        Command::new("tar")
            .args([
                "-czf",
                &archive.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        // Cap at 1024 bytes — the archive declares 2048
        let err = check_extraction_size(&archive, "tgz", 1024).unwrap_err();
        assert!(err.to_string().contains("declared size"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn size_cap_rejects_oversized_zip() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("big.bin"), vec![0u8; 2048]).unwrap();

        let archive = dir.path().join("test.zip");
        Command::new("zip")
            .args([
                "-j",
                &archive.display().to_string(),
                &src.join("big.bin").display().to_string(),
            ])
            .output()
            .unwrap();

        // Cap at 1024 bytes — the zip declares 2048
        let err = check_extraction_size(&archive, "zip", 1024).unwrap_err();
        assert!(err.to_string().contains("declared size"), "got: {err}");
    }

    #[test]
    fn invalid_zip_returns_error() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("bad.zip");
        fs::write(&archive, b"not a zip file").unwrap();

        let err = check_extraction_size(&archive, "zip", u64::MAX).unwrap_err();
        assert!(err.to_string().contains("failed to read zip archive"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn truncated_tar_is_rejected() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("truncated.tar");

        // Header declares 512 bytes but body is truncated (only 256 bytes)
        let mut f = fs::File::create(&archive).unwrap();
        let header = raw_tar_header(b"file.bin", 512);
        f.write_all(&header).unwrap();
        f.write_all(&[0u8; 256]).unwrap(); // truncated body

        // Truncated archives are rejected even when declared size is within cap
        assert!(check_extraction_size(&archive, "tar", 1024).is_err());
    }

    #[test]
    fn empty_reader_returns_zero() {
        // An empty reader (not a valid tar) should produce an error or zero size
        let data: &[u8] = &[];
        let archive = tar::Archive::new(data);
        // Empty archive returns 0 (no entries)
        let result = inspect_tar_entries(archive, u64::MAX);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 0);
    }

    #[test]
    fn native_tar_rejects_empty_bundle() {
        // A 0-byte file must error (parity with system `tar`), not be accepted
        // as a valid empty archive the way the `tar` crate would.
        let dir = TempDir::new().unwrap();
        let empty = dir.path().join("empty.tar");
        fs::write(&empty, b"").unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        let err = unpack_tar_native(&empty, &dest, false, false).unwrap_err();
        assert!(err.to_string().contains("empty"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn native_tar_extracts_files_and_dirs() {
        let dir = TempDir::new().unwrap();

        let src = dir.path().join("src");
        fs::create_dir_all(src.join("subdir")).unwrap();
        fs::write(src.join("root.txt"), "top-level").unwrap();
        fs::write(src.join("subdir/hello.txt"), "world").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_tar_native(&tar_path, &dest, false, false).unwrap();

        assert_eq!(fs::read_to_string(dest.join("root.txt")).unwrap(), "top-level");
        assert_eq!(fs::read_to_string(dest.join("subdir/hello.txt")).unwrap(), "world");
    }

    #[cfg(unix)]
    #[test]
    fn native_tgz_extracts_files() {
        let dir = TempDir::new().unwrap();

        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let tgz_path = dir.path().join("bundle.tgz");
        Command::new("tar")
            .args([
                "-czf",
                &tgz_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_tar_native(&tgz_path, &dest, true, false).unwrap();

        assert_eq!(fs::read_to_string(dest.join("hello.txt")).unwrap(), "world");
    }

    #[cfg(unix)]
    #[test]
    fn native_tar_preserves_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        let script = src.join("script.sh");
        fs::write(&script, "#!/bin/sh\necho hi").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_tar_native(&tar_path, &dest, false, false).unwrap();

        let mode = fs::metadata(dest.join("script.sh")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn native_tar_skips_traversal_entries() {
        // The extractor silently *skips* `..` entries (parity with system tar);
        // it is not the enforcement point. Traversal *rejection* that fails the
        // deployment lives in `check_path_traversal` (pre-extraction header scan)
        // and `reject_bundle_path_traversal` (post-extraction tree scan), both
        // gated on `reject_path_traversal_in_bundle`.
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("evil.tar");

        let mut f = fs::File::create(&archive).unwrap();
        let body = b"safe";
        f.write_all(&raw_tar_header(b"safe.txt", body.len() as u64)).unwrap();
        f.write_all(body).unwrap();
        f.write_all(&[0u8; 512 - 4]).unwrap();
        let bad = b"bad";
        f.write_all(&raw_tar_header(b"../../escaped.txt", bad.len() as u64)).unwrap();
        f.write_all(bad).unwrap();
        f.write_all(&[0u8; 512 - 3]).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_tar_native(&archive, &dest, false, false).unwrap();

        assert!(dest.join("safe.txt").exists());
        assert!(!dest.parent().unwrap().join("escaped.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn native_tar_preserves_symlink_entries() {
        // Parity with system `tar`: symlinks are extracted faithfully. Rejection
        // is the post-extraction scan's job, gated on reject_symlinks_in_bundle.
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("real.txt"), "data").unwrap();
        // Relative, in-bundle symlink target so it stays inside dest.
        std::os::unix::fs::symlink("real.txt", src.join("link")).unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_tar_native(&tar_path, &dest, false, false).unwrap();

        assert!(dest.join("real.txt").exists());
        let link_meta = dest.join("link").symlink_metadata().unwrap();
        assert!(link_meta.file_type().is_symlink(), "symlink entry must be preserved");
        // The post-extraction scan is what rejects it when the flag is on.
        let err = scan_bundle_for_links(&dest).unwrap_err();
        assert!(err.to_string().contains("symbolic link"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn native_tar_symlink_cannot_escape_dest_during_extraction() {
        // Mirror of `native_zip_symlink_cannot_escape_dest_during_extraction`
        // for the native tar fallback: a symlink entry pointing outside `dest`,
        // followed by a file entry that writes *through* it, must not escape
        // `dest` during extraction. `Entry::unpack_in` is responsible for
        // refusing the write-through; this test locks that guarantee in.
        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let canary = outside.join("canary.txt");

        // Build the archive with the tar crate so the entry ordering is exact:
        // "evil" -> <outside> (absolute symlink), then "evil/canary.txt".
        let archive_path = dir.path().join("escape.tar");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let mut builder = tar::Builder::new(file);

            let mut link_header = tar::Header::new_gnu();
            link_header.set_entry_type(tar::EntryType::Symlink);
            link_header.set_size(0);
            link_header.set_mode(0o777);
            builder.append_link(&mut link_header, "evil", &outside).unwrap();

            let body = b"pwned";
            let mut file_header = tar::Header::new_gnu();
            file_header.set_entry_type(tar::EntryType::Regular);
            file_header.set_size(body.len() as u64);
            file_header.set_mode(0o644);
            builder.append_data(&mut file_header, "evil/canary.txt", &body[..]).unwrap();

            builder.finish().unwrap();
        }

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        // Extraction may error or skip the through-symlink write — either is
        // acceptable. What must hold is that nothing escapes `dest`.
        let _ = unpack_tar_native(&archive_path, &dest, false, false);

        assert!(!canary.exists(), "write must not escape dest through a symlink");
    }

    #[cfg(unix)]
    #[test]
    fn native_tar_symlink_cannot_create_dirs_outside_dest() {
        // Mirror of `native_zip_symlink_cannot_create_dirs_outside_dest`: a
        // symlink entry pointing outside `dest`, followed by a nested entry,
        // must not create intermediate directories outside `dest` by resolving
        // through the symlink.
        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();

        let archive_path = dir.path().join("escape.tar");
        {
            let file = fs::File::create(&archive_path).unwrap();
            let mut builder = tar::Builder::new(file);

            let mut link_header = tar::Header::new_gnu();
            link_header.set_entry_type(tar::EntryType::Symlink);
            link_header.set_size(0);
            link_header.set_mode(0o777);
            builder.append_link(&mut link_header, "evil", &outside).unwrap();

            let body = b"pwned";
            let mut file_header = tar::Header::new_gnu();
            file_header.set_entry_type(tar::EntryType::Regular);
            file_header.set_size(body.len() as u64);
            file_header.set_mode(0o644);
            builder
                .append_data(&mut file_header, "evil/subdir/file.txt", &body[..])
                .unwrap();

            builder.finish().unwrap();
        }

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        let _ = unpack_tar_native(&archive_path, &dest, false, false);

        assert!(!outside.join("subdir").exists(), "no dir may be created outside dest");
    }

    #[test]
    fn native_tar_rejects_invalid_archive() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("bad.tar");
        // A non-empty, non-tar payload: header parse yields an error rather than
        // a clean end-of-archive.
        fs::write(&archive, vec![0x42u8; 1024]).unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        // Either the header parse errors, or it is treated as empty — both are
        // safe (no files extracted). Assert nothing escaped.
        let _ = unpack_tar_native(&archive, &dest, false, false);
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 0);
    }

    #[test]
    fn native_zip_extracts_files_and_dirs() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("test.zip");

        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);

        zip.add_directory("subdir/", zip::write::SimpleFileOptions::default()).unwrap();
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("subdir/hello.txt", opts).unwrap();
        zip.write_all(b"world").unwrap();
        zip.start_file("root.txt", opts).unwrap();
        zip.write_all(b"top-level").unwrap();
        zip.finish().unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_zip_native(&archive, &dest).unwrap();

        assert_eq!(fs::read_to_string(dest.join("subdir/hello.txt")).unwrap(), "world");
        assert_eq!(fs::read_to_string(dest.join("root.txt")).unwrap(), "top-level");
    }

    #[test]
    fn native_zip_skips_traversal_paths() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("evil.zip");

        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("safe.txt", opts).unwrap();
        zip.write_all(b"ok").unwrap();
        // enclosed_name() returns None for paths with traversal
        zip.start_file("../../../tmp/escaped.txt", opts).unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        // Must NOT fail the deployment on a traversal entry — matches system
        // unzip/tar, which strip the leading `../` rather than aborting.
        unpack_zip_native(&archive, &dest).unwrap();

        assert!(dest.join("safe.txt").exists());
        // The traversal entry is dropped and nothing escapes dest.
        assert!(!Path::new("/tmp/escaped.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn native_zip_preserves_symlink_entries() {
        // Parity with system `unzip`/native tar: symlinks are created as real
        // symlinks (not flattened to files). Rejection is the post-extraction
        // scan's job, gated on reject_symlinks_in_bundle.
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("link.zip");

        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("real.txt", opts).unwrap();
        zip.write_all(b"data").unwrap();
        // Relative, in-bundle target so it stays inside dest.
        zip.add_symlink("link", "real.txt", opts).unwrap();
        zip.finish().unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_zip_native(&archive, &dest).unwrap();

        assert!(dest.join("real.txt").exists());
        let link_meta = dest.join("link").symlink_metadata().unwrap();
        assert!(link_meta.file_type().is_symlink(), "symlink entry must be preserved");
        // The post-extraction scan is what rejects it when the flag is on.
        let err = scan_bundle_for_links(&dest).unwrap_err();
        assert!(err.to_string().contains("symbolic link"), "got: {err}");
    }

    #[cfg(unix)]
    #[test]
    fn native_zip_symlink_cannot_escape_dest_during_extraction() {
        // A symlink entry pointing outside dest, followed by a file entry that
        // writes *through* it, must not escape dest during extraction. The
        // guarded per-entry loop drops the escaping write; the post-scan still
        // rejects the bundle's symlink afterward when the flag is on.
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let canary = outside.join("canary.txt");

        let archive = dir.path().join("escape.zip");
        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        // "evil" -> absolute path outside dest
        zip.add_symlink("evil", outside.to_str().unwrap(), opts).unwrap();
        // then try to write through it
        zip.start_file("evil/canary.txt", opts).unwrap();
        zip.write_all(b"pwned").unwrap();
        zip.finish().unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_zip_native(&archive, &dest).unwrap();

        assert!(!canary.exists(), "write must not escape dest through a symlink");
    }

    #[cfg(unix)]
    #[test]
    fn native_zip_symlink_cannot_create_dirs_outside_dest() {
        // A symlink entry pointing outside dest, followed by a nested entry,
        // must not create intermediate directories outside dest via
        // create_dir_all resolving through the symlink.
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();

        let archive = dir.path().join("escape.zip");
        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.add_symlink("evil", outside.to_str().unwrap(), opts).unwrap();
        zip.start_file("evil/subdir/file.txt", opts).unwrap();
        zip.write_all(b"pwned").unwrap();
        zip.finish().unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_zip_native(&archive, &dest).unwrap();

        assert!(!outside.join("subdir").exists(), "no dir may be created outside dest");
    }

    #[cfg(unix)]
    #[test]
    fn native_zip_symlink_leaf_write_through_cannot_escape_dest() {
        // Regression for the symlink-write-through-on-the-leaf finding: two
        // entries whose raw names DIFFER ("payload" and "./payload") so the zip
        // crate does not dedup them, but whose enclosed names both normalize to
        // the same `out_path` (dest/payload). Entry 1 is a symlink pointing at an
        // external file; entry 2 is a regular file. Without the leaf guard,
        // `File::create(dest/payload)` follows the symlink and writes through to
        // the external target. The guard must remove the leaf symlink first so
        // the write lands inside `dest`.
        //
        // (Same raw names would be rejected as "Duplicate filename" by ZipWriter
        // and silently collapsed to one entry by the reader — hence the `./`.)
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join("backdoor");
        fs::write(&target, b"original").unwrap();

        let archive = dir.path().join("leaf.zip");
        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        // entry 1: symlink "payload" -> external target
        zip.add_symlink("payload", target.to_str().unwrap(), opts).unwrap();
        // entry 2: regular file with a distinct raw name that normalizes to "payload"
        zip.start_file("./payload", opts).unwrap();
        zip.write_all(b"PWNED").unwrap();
        zip.finish().unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_zip_native(&archive, &dest).unwrap();

        // The external target must be untouched...
        assert_eq!(
            fs::read(&target).unwrap(),
            b"original",
            "write escaped dest through a leaf symlink"
        );
        // ...and the regular-file content must have landed inside dest instead.
        assert_eq!(fs::read(dest.join("payload")).unwrap(), b"PWNED");
        assert!(
            !dest.join("payload").symlink_metadata().unwrap().file_type().is_symlink(),
            "leaf symlink should have been replaced by the regular file"
        );
    }

    #[cfg(unix)]
    #[test]
    fn native_zip_preserves_unix_permissions() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("perms.zip");

        let file = fs::File::create(&archive).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default().unix_permissions(0o755);
        zip.start_file("script.sh", opts).unwrap();
        zip.write_all(b"#!/bin/sh\necho hi").unwrap();
        zip.finish().unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        unpack_zip_native(&archive, &dest).unwrap();

        let mode = fs::metadata(dest.join("script.sh")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755);
    }

    #[test]
    fn native_zip_rejects_invalid_archive() {
        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("bad.zip");
        fs::write(&archive, b"not a zip").unwrap();

        let dest = dir.path().join("out");
        fs::create_dir_all(&dest).unwrap();
        let err = unpack_zip_native(&archive, &dest).unwrap_err();
        assert!(err.to_string().contains("failed to read zip archive"));
    }

    #[test]
    fn size_cap_rejects_oversized_zip_native() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("big.zip");

        let file = fs::File::create(&archive).expect("create zip file");
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("big.bin", opts).expect("start zip entry");
        zip.write_all(&[0u8; 4096]).expect("write zip content");
        zip.finish().expect("finish zip");

        let err = check_extraction_size(&archive, "zip", 1024).unwrap_err();
        assert!(err.to_string().contains("declared size"), "got: {err}");
    }

    #[test]
    fn size_cap_allows_zip_under_limit() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("small.zip");

        let file = fs::File::create(&archive).expect("create zip file");
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file("small.txt", opts).expect("start zip entry");
        zip.write_all(b"hello").expect("write zip content");
        zip.finish().expect("finish zip");

        assert!(check_extraction_size(&archive, "zip", 1024).is_ok());
    }

    #[test]
    fn check_path_traversal_rejects_parent_in_plain_tar() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archive = dir.path().join("evil.tar");

        let mut f = fs::File::create(&archive).expect("create tar file");
        let body = b"x";
        f.write_all(&raw_tar_header(b"../escape.txt", body.len() as u64))
            .expect("write tar header");
        f.write_all(body).expect("write body");
        f.write_all(&[0u8; 511]).expect("write padding");
        f.write_all(&[0u8; 1024]).expect("write end-of-archive");

        // Use bundle_type "tar" (not "tgz") to exercise the non-gzip branch
        let err = check_path_traversal(&archive, "tar").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("traversal component"), "got: {msg}");
    }
}
