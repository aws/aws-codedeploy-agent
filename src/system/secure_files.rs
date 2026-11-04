//! Helpers for creating agent-owned files and directories with explicit,
//! umask-independent permissions.
//!
//! On Unix, applies POSIX modes directly. On Windows, applies a protected
//! DACL granting `GENERIC_ALL` to SYSTEM and BUILTIN\Administrators only
//! (the `mode` parameter is ignored — Windows uses a single restrictive ACL).
//!
//! Note: the Windows path is non-preserving — each call unconditionally resets
//! the DACL to SYSTEM + Administrators. Unlike Unix (which preserves
//! operator-set tighter modes), any custom ACEs added by an operator will be
//! overwritten on agent restart.

use std::io;
use std::path::Path;

/// Build a security descriptor from the standard agent SDDL string.
/// Returns the SD pointer (caller must `LocalFree` it).
///
/// SDDL `D:P(A;;GA;;;SY)(A;;GA;;;BA)`:
/// - `D:P` = protected DACL (blocks inheritance from parent)
/// - `(A;;GA;;;SY)` = GENERIC_ALL → SYSTEM
/// - `(A;;GA;;;BA)` = GENERIC_ALL → BUILTIN\Administrators
#[cfg(windows)]
#[allow(unsafe_code)]
fn agent_security_descriptor() -> io::Result<*mut core::ffi::c_void> {
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;

    let sddl: Vec<u16> = "D:P(A;;GA;;;SY)(A;;GA;;;BA)\0".encode_utf16().collect();
    let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();

    // SAFETY: `sddl` is a NUL-terminated UTF-16 literal; `&mut sd` is a valid
    // out-pointer; `null_mut()` for size is documented as "don't return size".
    let ret = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            1, // SDDL_REVISION_1
            &mut sd,
            std::ptr::null_mut(),
        )
    };
    if ret == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(sd)
}

/// Apply the agent DACL to an existing file or directory.
#[cfg(windows)]
#[allow(unsafe_code)]
fn apply_agent_dacl(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::{
        DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION, SetFileSecurityW,
    };

    let sd = agent_security_descriptor()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    // SAFETY: `wide` is NUL-terminated; `sd` is a valid self-relative security
    // descriptor from `ConvertStringSecurityDescriptorToSecurityDescriptorW`.
    let ret = unsafe {
        SetFileSecurityW(
            wide.as_ptr(),
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            sd,
        )
    };

    // Capture error before LocalFree can clobber last-error.
    let err = if ret == 0 {
        Some(io::Error::last_os_error())
    } else {
        None
    };

    // SAFETY: `sd` was allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`
    // and must be freed exactly once.
    unsafe { LocalFree(sd) };

    if let Some(e) = err {
        return Err(e);
    }
    Ok(())
}

/// Grant `BUILTIN\Users` read and execute on a directory, inherited by the
/// files and subdirectories under it, keeping the directory's other ACEs.
/// Never grants write. A no-op when Users can already read the directory
/// (for example through an ACE inherited from an agent dir above it), so
/// re-touching a dir doesn't re-propagate ACLs through its whole subtree.
///
/// This is the Windows counterpart of the Unix 0755 parity mode: the MSI
/// locks the install root to SYSTEM + Administrators, so agent dirs no
/// longer inherit the `%ProgramData%` Users read ACE and need it explicitly.
#[cfg(windows)]
#[allow(unsafe_code)]
fn grant_users_read(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Foundation::{ERROR_SUCCESS, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        EXPLICIT_ACCESS_W, GRANT_ACCESS, GetEffectiveRightsFromAclW, GetNamedSecurityInfoW,
        NO_MULTIPLE_TRUSTEE, SE_FILE_OBJECT, SetEntriesInAclW, SetNamedSecurityInfoW,
        TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
    };
    use windows_sys::Win32::Security::{
        ACL, CreateWellKnownSid, DACL_SECURITY_INFORMATION, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
        WinBuiltinUsersSid,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_GENERIC_EXECUTE, FILE_GENERIC_READ};

    const SECURITY_MAX_SID_SIZE: u32 = 68;
    let read_execute = FILE_GENERIC_READ | FILE_GENERIC_EXECUTE;

    let mut sid = [0u8; SECURITY_MAX_SID_SIZE as usize];
    let mut sid_len = SECURITY_MAX_SID_SIZE;
    // SAFETY: `sid` is a writable buffer of `sid_len` bytes; a null domain
    // SID is valid for a BUILTIN well-known SID.
    if unsafe {
        CreateWellKnownSid(
            WinBuiltinUsersSid,
            std::ptr::null_mut(),
            sid.as_mut_ptr().cast(),
            &mut sid_len,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let trustee = TRUSTEE_W {
        pMultipleTrustee: std::ptr::null_mut(),
        MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
        ptstrName: sid.as_mut_ptr().cast(),
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `wide` is NUL-terminated; the out-pointers are valid. `dacl`
    // points into `sd`, which is freed below with `LocalFree`.
    let ret = unsafe {
        GetNamedSecurityInfoW(
            wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut dacl,
            std::ptr::null_mut(),
            &mut sd,
        )
    };
    if ret != ERROR_SUCCESS {
        return Err(win32_error(ret));
    }

    let mut current: u32 = 0;
    // SAFETY: `dacl` comes from `GetNamedSecurityInfoW` and `trustee` holds a
    // valid SID that outlives the call.
    let have = unsafe { GetEffectiveRightsFromAclW(dacl, &trustee, &mut current) };
    let result = if have == ERROR_SUCCESS && current & read_execute == read_execute {
        Ok(())
    } else {
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: read_execute,
            grfAccessMode: GRANT_ACCESS,
            grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            Trustee: trustee,
        };
        let mut new_dacl: *mut ACL = std::ptr::null_mut();
        // SAFETY: `entry` is a valid EXPLICIT_ACCESS_W; `dacl` is the
        // existing DACL; `new_dacl` is a valid out-pointer.
        let ret = unsafe { SetEntriesInAclW(1, &entry, dacl, &mut new_dacl) };
        if ret == ERROR_SUCCESS {
            // DACL_SECURITY_INFORMATION alone keeps the dir's protection
            // state: a protected DACL stays protected, an inheriting one
            // keeps inheriting.
            // SAFETY: `wide` is NUL-terminated; `new_dacl` is a valid ACL from
            // `SetEntriesInAclW`.
            let ret = unsafe {
                SetNamedSecurityInfoW(
                    wide.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    new_dacl,
                    std::ptr::null(),
                )
            };
            // SAFETY: `new_dacl` was allocated by `SetEntriesInAclW`.
            unsafe { LocalFree(new_dacl.cast()) };
            if ret == ERROR_SUCCESS {
                Ok(())
            } else {
                Err(win32_error(ret))
            }
        } else {
            Err(win32_error(ret))
        }
    };

    // SAFETY: `sd` was allocated by `GetNamedSecurityInfoW` and is freed once.
    unsafe { LocalFree(sd) };
    result
}

/// Convert a `WIN32_ERROR` return code (the security APIs return it rather
/// than setting last-error) into an `io::Error`.
#[cfg(windows)]
fn win32_error(code: u32) -> io::Error {
    i32::try_from(code).map_or_else(
        |_| io::Error::other(format!("Win32 error {code}")),
        io::Error::from_raw_os_error,
    )
}

/// Create a file with the agent DACL applied atomically via
/// `CreateFileW` + `SECURITY_ATTRIBUTES`. Returns the open handle wrapped
/// in a `std::fs::File`.
///
/// `creation_disposition` controls create-vs-truncate behavior (e.g.
/// `CREATE_NEW` for tempfiles, `CREATE_ALWAYS` for overwrite-or-create).
#[cfg(windows)]
#[allow(unsafe_code)]
fn create_file_with_dacl(path: &Path, creation_disposition: u32) -> io::Result<std::fs::File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;

    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE,
    };

    let sd = agent_security_descriptor()?;
    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    // SAFETY: `wide` is NUL-terminated; `&sa` lives for the call;
    // return value checked against INVALID_HANDLE_VALUE.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            FILE_GENERIC_WRITE,
            0,
            &sa,
            creation_disposition,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        )
    };

    // Capture last-error before LocalFree can clobber it.
    let create_err = if handle == INVALID_HANDLE_VALUE {
        Some(io::Error::last_os_error())
    } else {
        None
    };

    // SAFETY: `sd` allocated by ConvertString..., freed exactly once.
    unsafe { LocalFree(sd) };

    if let Some(err) = create_err {
        return Err(err);
    }

    // SAFETY: handle validated (not INVALID_HANDLE_VALUE), no other references.
    Ok(unsafe { std::fs::File::from_raw_handle(handle as *mut core::ffi::c_void) })
}

/// Create `path` and any missing parents, applying exactly `mode` to every
/// component this call creates. Returns whether `path` already existed.
///
/// `mkdir(2)` filters its mode argument through the process umask (the
/// master hardens it to 0o027 — see `main.rs`), and `DirBuilder`'s
/// `recursive(true)` offers no way to chmod the intermediates it creates.
/// Chmod'ing only the leaf afterward leaves parents umask-filtered: with
/// `root_dir: /export/deployment-root` on a fresh host, `/export` is born
/// 0750 `root:root`, so every non-root process — including `runas:` hook
/// execution, which may resolve through symlinks under those parents — gets
/// EACCES on the first path component. Creating each missing component
/// individually and chmod'ing it immediately closes that gap.
///
/// Pre-existing ancestors are never modified — the agent cannot tell an
/// operator-tightened parent (deliberate) from a stale agent-created one,
/// so it only sets modes on directories it creates itself.
#[cfg(unix)]
fn create_dir_all_exact_mode(path: &Path, mode: u32) -> io::Result<bool> {
    use std::fs::{DirBuilder, Permissions};
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    if path.is_dir() {
        return Ok(true);
    }

    // Collect the missing components, leaf first.
    let mut missing = Vec::new();
    let mut current = path;
    while !current.exists() {
        missing.push(current.to_path_buf());
        match current.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => current = parent,
            _ => break,
        }
    }

    let mut builder = DirBuilder::new();
    builder.mode(mode);
    // Create root-most first so each mkdir's parent exists.
    for component in missing.iter().rev() {
        match builder.create(component) {
            // mkdir(2) applied the umask; force the exact mode on the
            // component we just created.
            Ok(()) => std::fs::set_permissions(component, Permissions::from_mode(mode))?,
            // Tolerate a concurrent creation of the same component.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {},
            Err(e) => return Err(e),
        }
    }
    Ok(false)
}

/// Create a directory (and any missing parents) with the exact `mode`,
/// independent of the process umask. Every component created by this call
/// gets exactly `mode`; pre-existing ancestors are never modified.
///
/// On Windows, applies a protected DACL (SYSTEM + Administrators only)
/// regardless of the `mode` value.
///
/// # Errors
/// Returns an error if directory creation or permission setting fails.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn create_dir_secure(path: &Path, mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;

        let existed = create_dir_all_exact_mode(path, mode)?;
        if existed {
            // Tighten existing directories that are more permissive than
            // requested, while preserving operator overrides that are *more*
            // restrictive (e.g., operator set 0600 and we request 0700 →
            // result is 0600). Handles upgrade from older agent versions
            // that created dirs at umask-default 0755, and self-heals
            // permission drift between restarts.
            // Only tighten directories we own — avoids EPERM on system dirs
            // like /tmp that happen to be a parent in test environments.
            let meta = std::fs::metadata(path)?;
            let current = meta.permissions().mode() & 0o777;
            let we_own_it = {
                use std::os::unix::fs::MetadataExt;
                meta.uid() == nix::unistd::getuid().as_raw()
            };
            if we_own_it && (current & !mode) != 0 {
                std::fs::set_permissions(path, Permissions::from_mode(current & mode))?;
            }
        } else {
            std::fs::set_permissions(path, Permissions::from_mode(mode))?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(path)?;
        apply_agent_dacl(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        compile_error!("secure_files requires Unix or Windows");
    }
}

/// Agent-state file mode for the given `restrict_agent_dir_permissions`
/// policy: 0644 (world-readable) by default for backwards compatibility, 0600
/// under opt-in hardening. Applies to the PID file, deployment tracking files,
/// instruction files, markers, and downloaded bundles.
#[must_use]
pub fn agent_file_mode(restrict: bool) -> u32 {
    if restrict { 0o600 } else { 0o644 }
}

/// Create a directory (and any missing parents) world-readable at exactly
/// 0755, independent of the process umask, force-loosening a pre-existing
/// directory that is more restrictive.
///
/// This is the backwards-compatible counterpart to [`create_dir_secure`]:
/// deployment-root directories have historically been world-readable 0755, and
/// host tooling outside the agent depends on being able to read and traverse
/// them — tightening them to 0700 makes such processes fail with EACCES. The
/// force-loosen matters on upgrade: a host that already ran a tightened agent
/// has these dirs at 0700/0711, and [`create_dir_secure`] only ever tightens.
///
/// On Windows, creates the directory with its inherited ACL and grants
/// `BUILTIN\Users` read and execute, inherited by everything under it (see
/// [`grant_users_read`]). Users never get write. A pre-existing protected
/// DACL from an earlier agent version stays protected but gains the Users
/// read ACE.
///
/// # Errors
/// Returns an error if directory creation or permission setting fails.
pub fn create_dir_world_readable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        // Applies exactly 0755 to every component this call creates —
        // including intermediate parents (e.g. `/export` when `root_dir` is
        // `/export/deployment-root` on a fresh host), which `recursive(true)`
        // would otherwise leave umask-filtered at 0750 under the master's
        // 0o027, making the parent non-traversable for other processes.
        create_dir_all_exact_mode(path, 0o755)?;
        // A pre-existing leaf keeps its old mode — ensure the 0o755 bits are
        // present (upgrade healing from a previously tightened agent).
        // Loosen-only (OR, never mask): a dir that is already *more*
        // permissive (e.g. /tmp at 1777 when a caller's path sits directly
        // inside it) must not lose write/sticky/setgid bits. Only chmod dirs
        // we own, mirroring `create_dir_secure` — avoids EPERM on system
        // parents in test environments.
        let meta = std::fs::metadata(path)?;
        let current = meta.permissions().mode();
        let we_own_it = meta.uid() == nix::unistd::getuid().as_raw();
        let target = current | 0o755;
        if we_own_it && current & 0o7777 != target & 0o7777 {
            std::fs::set_permissions(path, Permissions::from_mode(target))?;
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        std::fs::create_dir_all(path)?;
        grant_users_read(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        compile_error!("secure_files requires Unix or Windows");
    }
}

/// Create a deployment-root directory with the mode policy selected by the
/// `restrict_agent_dir_permissions` config flag.
///
/// `restrict = false` (default, backwards-compatible): world-readable 0755 via
/// [`create_dir_world_readable`], force-loosening a dir a tightened agent
/// left at 0700/0711 — upgrades must restore access for host tooling without
/// an instance rotation.
///
/// `restrict = true` (opt-in hardening): exactly `hardened_mode`, force-set
/// in both directions so flipping the flag on an existing install converges.
/// On Windows this applies the protected SYSTEM+Administrators DACL.
///
/// # Errors
/// Returns an error if directory creation or permission setting fails.
pub fn create_deployment_dir(path: &Path, hardened_mode: u32, restrict: bool) -> io::Result<()> {
    if restrict {
        create_dir_secure(path, hardened_mode)?;
        // `create_dir_secure` only tightens an existing dir; force the exact
        // mode so dirs created under the default 0755 policy converge.
        #[cfg(unix)]
        {
            use std::fs::Permissions;
            use std::os::unix::fs::PermissionsExt;
            let current = std::fs::metadata(path)?.permissions().mode() & 0o777;
            if current != hardened_mode {
                std::fs::set_permissions(path, Permissions::from_mode(hardened_mode))?;
            }
        }
        Ok(())
    } else {
        create_dir_world_readable(path)
    }
}

/// Create the agent `root_dir` with the policy selected by
/// `restrict_agent_dir_permissions`. Same as [`create_deployment_dir`],
/// except on Windows under the parity default: there `root_dir` is the MSI
/// install root (`%PROGRAMDATA%\Amazon\CodeDeploy`), which also holds `bin\`,
/// `conf.yml` and `conf.onpremises.yml`, so its installer ACL (SYSTEM +
/// Administrators) is left as is. The agent dirs under it get Users read
/// from [`create_dir_world_readable`].
///
/// # Errors
/// Returns an error if directory creation or permission setting fails.
pub fn create_root_dir(path: &Path, hardened_mode: u32, restrict: bool) -> io::Result<()> {
    #[cfg(windows)]
    if !restrict {
        return std::fs::create_dir_all(path);
    }
    create_deployment_dir(path, hardened_mode, restrict)
}

/// Write `content` to `path` atomically, with the file created at exactly
/// `mode` regardless of process umask.
///
/// On Unix, uses a tempfile-in-same-directory + `sync_all` + rename pattern
/// so concurrent readers always see a fully written file.
///
/// On Windows, uses a tempfile-in-same-directory + `sync_all` +
/// `MoveFileExW(MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)` for
/// equivalent crash-safety. The tempfile is created with a protected DACL
/// (SYSTEM + Administrators only) via `CreateFileW` + `SECURITY_ATTRIBUTES`,
/// and the DACL survives the move.
///
/// # Errors
/// Returns an error if the parent directory does not exist, or if the write
/// or rename fails.
#[cfg_attr(not(unix), allow(unused_variables))]
#[cfg_attr(windows, allow(unsafe_code))]
pub fn write_file_secure(path: &Path, content: &[u8], mode: u32) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::fs::{OpenOptions, Permissions};
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
        })?;
        let file_name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;

        let pid = std::process::id();
        let tmp_name = format!(".{}.tmp.{pid}", file_name.to_string_lossy());
        let tmp_path = parent.join(&tmp_name);

        let mut file =
            match OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp_path) {
                Ok(f) => f,
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    std::fs::remove_file(&tmp_path)?;
                    OpenOptions::new().write(true).create_new(true).mode(mode).open(&tmp_path)?
                },
                Err(e) => return Err(e),
            };

        // fchmod on the open fd — umask-independent, no TOCTOU.
        file.set_permissions(Permissions::from_mode(mode))?;

        let write_result = file.write_all(content).and_then(|()| file.sync_all());
        drop(file);

        if let Err(e) = write_result {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }

        if let Err(e) = std::fs::rename(&tmp_path, path) {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }
        Ok(())
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;

        use windows_sys::Win32::Storage::FileSystem::{
            CREATE_NEW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
        };

        let parent = path.parent().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "path has no parent directory")
        })?;
        let file_name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?;

        let pid = std::process::id();
        let tmp_name = format!(".{}.tmp.{pid}", file_name.to_string_lossy());
        let tmp_path = parent.join(&tmp_name);

        // Clean up stale tempfile from a prior crash.
        match std::fs::remove_file(&tmp_path) {
            Ok(()) => {},
            Err(e) if e.kind() == io::ErrorKind::NotFound => {},
            Err(e) => return Err(e),
        }

        // Create tempfile with agent DACL applied atomically.
        let mut file = create_file_with_dacl(&tmp_path, CREATE_NEW)?;

        let write_result =
            std::io::Write::write_all(&mut file, content).and_then(|()| file.sync_all());
        drop(file);

        if let Err(e) = write_result {
            let _ = std::fs::remove_file(&tmp_path);
            return Err(e);
        }

        // Atomic replace: MOVEFILE_REPLACE_EXISTING overwrites the target;
        // MOVEFILE_WRITE_THROUGH flushes the move to disk before returning.
        // The DACL from the tempfile survives the move (NTFS preserves the
        // source file's security descriptor on same-volume renames).
        let wide_src: Vec<u16> =
            tmp_path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        let wide_dst: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

        // SAFETY: both paths are NUL-terminated; flags are valid constants.
        let ret = unsafe {
            MoveFileExW(
                wide_src.as_ptr(),
                wide_dst.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        };
        if ret == 0 {
            let err = io::Error::last_os_error();
            let _ = std::fs::remove_file(&tmp_path);
            return Err(err);
        }
        Ok(())
    }
    #[cfg(not(any(unix, windows)))]
    {
        compile_error!("secure_files requires Unix or Windows");
    }
}

/// Create a file for writing with an explicit, umask-independent mode.
/// Returns the open `File` handle so callers can stream data into it.
///
/// Use this when you need a `File` handle for streaming writes; for
/// fully-in-memory content prefer [`write_file_secure`] which writes
/// atomically via a tempfile.
///
/// On Unix, uses `OpenOptions::create(true).truncate(true)` preserving the
/// same inode if the file exists.
///
/// On Windows, removes any existing file then creates a new one with a
/// protected DACL (SYSTEM + Administrators only) via `CreateFileW` +
/// `SECURITY_ATTRIBUTES`. This produces a new file object (not a truncate),
/// so callers holding an open handle to the old file will see `AccessDenied`
/// on the remove step. The agent-owned state directory prevents races with
/// unprivileged users.
///
/// # Errors
/// Returns an error if the file cannot be created or its mode cannot be set.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn create_file_secure(path: &Path, mode: u32) -> io::Result<std::fs::File> {
    #[cfg(unix)]
    {
        use std::fs::{OpenOptions, Permissions};
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let f = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(mode)
            .open(path)?;
        // mode() only applies on creation; chmod covers the existing-file
        // case and any umask stripping.
        f.set_permissions(Permissions::from_mode(mode))?;
        Ok(f)
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Storage::FileSystem::CREATE_NEW;

        // Match write_file_secure: remove + CREATE_NEW applies SECURITY_ATTRIBUTES
        // to a fresh file.
        match std::fs::remove_file(path) {
            Ok(()) => {},
            Err(e) if e.kind() == io::ErrorKind::NotFound => {},
            Err(e) => return Err(e),
        }

        create_file_with_dacl(path, CREATE_NEW)
    }
    #[cfg(not(any(unix, windows)))]
    {
        compile_error!("secure_files requires Unix or Windows");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn create_dir_secure_sets_exact_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/deep/state");
        create_dir_secure(&path, 0o700).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "expected 0700, got {mode:#o}");
    }

    #[test]
    fn create_dir_secure_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state");
        create_dir_secure(&path, 0o700).unwrap();
        create_dir_secure(&path, 0o700).unwrap();
    }

    #[test]
    fn create_dir_secure_tightens_overly_permissive_existing_dir() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        create_dir_secure(&path, 0o700).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "expected 0700 after tightening, got {mode:#o}");
    }

    #[test]
    fn create_dir_secure_preserves_more_restrictive_operator_override() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        create_dir_secure(&path, 0o700).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600 preserved, got {mode:#o}");
    }

    #[test]
    #[serial_test::serial(umask)]
    fn create_dir_secure_ignores_umask() {
        use nix::sys::stat::{Mode, umask};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("restricted");
        let prev = umask(Mode::from_bits_truncate(0o077));
        let result = create_dir_secure(&path, 0o750);
        umask(prev);
        result.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o750, "expected 0750 despite umask 0077, got {mode:#o}");
    }

    /// Regression test: with the master's hardening umask (0o027) active and a
    /// multi-level missing path (`/export/deployment-root` where `/export`
    /// doesn't exist), every created component — not just the leaf — must get
    /// the exact requested mode. A umask-filtered 0750 parent is not
    /// traversable by other users and makes `runas:` hook execution fail with
    /// EACCES.
    #[test]
    #[serial_test::serial(umask)]
    fn create_dir_secure_applies_exact_mode_to_created_parents() {
        use nix::sys::stat::{Mode, umask};
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("export");
        let leaf = parent.join("deployment-root");
        let prev = umask(Mode::from_bits_truncate(0o027));
        let result = create_dir_secure(&leaf, 0o711);
        umask(prev);
        result.unwrap();
        let parent_mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        let leaf_mode = std::fs::metadata(&leaf).unwrap().permissions().mode() & 0o777;
        assert_eq!(parent_mode, 0o711, "created parent must get exact mode, got {parent_mode:#o}");
        assert_eq!(leaf_mode, 0o711, "leaf must get exact mode, got {leaf_mode:#o}");
    }

    /// Pre-existing ancestors must never be modified: the agent cannot
    /// distinguish an operator-tightened parent from a stale one, so it
    /// only sets modes on directories it creates itself.
    #[test]
    fn create_dir_secure_leaves_preexisting_parent_untouched() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("export");
        std::fs::create_dir(&parent).unwrap();
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        create_dir_secure(&parent.join("deployment-root"), 0o711).unwrap();
        let parent_mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            parent_mode, 0o700,
            "pre-existing parent must keep its mode, got {parent_mode:#o}"
        );
    }

    /// Regression test, default-policy variant: the non-restricted
    /// deployment-dir path must also produce traversable created parents under
    /// the master's umask.
    #[test]
    #[serial_test::serial(umask)]
    fn create_dir_world_readable_applies_0755_to_created_parents() {
        use nix::sys::stat::{Mode, umask};
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("export");
        let leaf = parent.join("deployment-root");
        let prev = umask(Mode::from_bits_truncate(0o027));
        let result = create_dir_world_readable(&leaf);
        umask(prev);
        result.unwrap();
        let parent_mode = std::fs::metadata(&parent).unwrap().permissions().mode() & 0o777;
        assert_eq!(parent_mode, 0o755, "created parent must be 0755, got {parent_mode:#o}");
    }

    #[test]
    fn create_dir_world_readable_sets_0755() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested/ongoing-deployment");
        create_dir_world_readable(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "expected 0755, got {mode:#o}");
    }

    #[test]
    fn create_dir_world_readable_loosens_preexisting_0700_dir() {
        // Upgrade path from a previously tightened agent: the dir already
        // exists at 0700 and must be force-loosened back to 0755.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ongoing-deployment");
        std::fs::create_dir(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        create_dir_world_readable(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "expected upgrade to loosen 0700 -> 0755, got {mode:#o}");
    }

    #[test]
    #[serial_test::serial(umask)]
    fn create_dir_world_readable_ignores_umask() {
        use nix::sys::stat::{Mode, umask};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("world-readable");
        let prev = umask(Mode::from_bits_truncate(0o027));
        let result = create_dir_world_readable(&path);
        umask(prev);
        result.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "expected 0755 despite umask 0027, got {mode:#o}");
    }

    #[test]
    #[serial_test::serial(umask)]
    fn write_file_secure_ignores_umask() {
        use nix::sys::stat::{Mode, umask};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("log.txt");
        let prev = umask(Mode::from_bits_truncate(0o077));
        let result = write_file_secure(&path, b"data", 0o640);
        umask(prev);
        result.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "expected 0640 despite umask 0077, got {mode:#o}");
    }

    #[test]
    fn write_file_secure_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        write_file_secure(&path, b"old", 0o600).unwrap();
        write_file_secure(&path, b"new", 0o600).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    fn write_file_secure_recovers_from_stale_tempfile() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        let pid = std::process::id();
        let tmp_path = tmp.path().join(format!(".{}.tmp.{pid}", "state.json"));
        std::fs::write(&tmp_path, b"garbage").unwrap();

        write_file_secure(&path, b"fresh", 0o600).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fresh");
        assert!(!tmp_path.exists());
    }

    #[test]
    fn write_file_secure_rejects_rootless_path() {
        let result = write_file_secure(Path::new("/"), b"x", 0o600);
        assert!(result.is_err());
    }

    #[test]
    fn create_file_secure_sets_exact_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("download.bin");
        let _ = create_file_secure(&path, 0o600).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:#o}");
    }

    #[test]
    fn create_file_secure_truncates_existing() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("download.bin");
        std::fs::write(&path, b"old").unwrap();
        let mut f = create_file_secure(&path, 0o600).unwrap();
        f.write_all(b"new").unwrap();
        drop(f);
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
    }

    #[test]
    #[serial_test::serial(umask)]
    fn create_file_secure_ignores_umask() {
        use nix::sys::stat::{Mode, umask};
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("download.bin");
        let prev = umask(Mode::from_bits_truncate(0o077));
        let result = create_file_secure(&path, 0o640);
        umask(prev);
        let _ = result.unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "expected 0640 despite umask 0077, got {mode:#o}");
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Full control access mask matching the SDDL `GA` (GENERIC_ALL).
    const GENERIC_ALL: u32 = 0x1000_0000;

    /// `FILE_ALL_ACCESS`: what Windows reports after mapping `GENERIC_ALL`
    /// to file-object-specific rights when the ACE is stored.
    const FILE_ALL_ACCESS: u32 = 0x001F_01FF;

    /// ACE flag indicating the entry was inherited from a parent object.
    const INHERITED_ACE: u8 = 0x10;

    /// Assert the path has exactly 2 AccessAllow entries (SYSTEM + Administrators),
    /// each with GENERIC_ALL, none inherited.
    fn assert_agent_dacl(path: &Path) {
        use windows_acl::acl::{ACL, AceType};

        let acl = ACL::from_file_path(path.to_str().unwrap(), false).unwrap();
        let entries = acl.all().unwrap();
        let allow: Vec<_> =
            entries.iter().filter(|e| e.entry_type == AceType::AccessAllow).collect();

        assert_eq!(allow.len(), 2, "expected exactly 2 AccessAllow entries, got {}", allow.len());

        for e in &allow {
            assert_eq!(
                e.flags & INHERITED_ACE,
                0,
                "entry {} has INHERITED_ACE flag — DACL is not protected",
                e.string_sid
            );
            assert!(
                e.mask == GENERIC_ALL || e.mask == FILE_ALL_ACCESS,
                "entry {} has mask {:#x}, expected GENERIC_ALL ({GENERIC_ALL:#x}) or its \
                 file-specific mapping FILE_ALL_ACCESS ({FILE_ALL_ACCESS:#x})",
                e.string_sid,
                e.mask
            );
        }

        let sids: Vec<&str> = allow.iter().map(|e| e.string_sid.as_str()).collect();
        assert!(sids.contains(&"S-1-5-18"), "SYSTEM not in DACL: {sids:?}");
        assert!(sids.contains(&"S-1-5-32-544"), "Administrators not in DACL: {sids:?}");
    }

    #[test]
    fn create_dir_secure_applies_dacl() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state");
        create_dir_secure(&path, 0o700).unwrap();
        assert_agent_dacl(&path);
    }

    #[test]
    fn create_dir_secure_reapplies_dacl_to_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state");
        // Simulate pre-upgrade directory with inherited (permissive) ACLs.
        std::fs::create_dir(&path).unwrap();
        // Second call should re-apply the protected DACL.
        create_dir_secure(&path, 0o700).unwrap();
        assert_agent_dacl(&path);
    }

    #[test]
    fn write_file_secure_applies_dacl_and_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        write_file_secure(&path, b"data", 0o600).unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"data");
        assert_agent_dacl(&path);
    }

    #[test]
    fn create_file_secure_applies_dacl() {
        use std::io::Write;

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("download.bin");
        let mut f = create_file_secure(&path, 0o600).unwrap();
        f.write_all(b"payload").unwrap();
        drop(f);

        assert_agent_dacl(&path);
    }

    #[test]
    fn write_file_secure_overwrites_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("state.json");
        write_file_secure(&path, b"old", 0o600).unwrap();
        write_file_secure(&path, b"new", 0o600).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new");
        assert_agent_dacl(&path);
    }

    /// SID of `BUILTIN\Users`.
    const USERS_SID: &str = "S-1-5-32-545";
    /// `FILE_GENERIC_READ | FILE_GENERIC_EXECUTE` (`ReadAndExecute`).
    const READ_EXECUTE: u32 = 0x0012_00A9;
    /// `FILE_WRITE_DATA`, `FILE_APPEND_DATA`, `FILE_WRITE_EA`,
    /// `FILE_WRITE_ATTRIBUTES`, `DELETE`, `WRITE_DAC` and `WRITE_OWNER`.
    const ANY_WRITE: u32 =
        0x0002 | 0x0004 | 0x0010 | 0x0100 | 0x0001_0000 | 0x0004_0000 | 0x0008_0000;
    /// `OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE`.
    const OI_CI: u8 = 0x01 | 0x02;

    /// Allow ACEs for `BUILTIN\Users` on `path`: `(mask, flags)` pairs.
    fn users_allow_aces(path: &Path) -> Vec<(u32, u8)> {
        use windows_acl::acl::{ACL, AceType};

        let acl = ACL::from_file_path(path.to_str().unwrap(), false).unwrap();
        acl.all()
            .unwrap()
            .iter()
            .filter(|e| e.entry_type == AceType::AccessAllow && e.string_sid == USERS_SID)
            .map(|e| (e.mask, e.flags))
            .collect()
    }

    #[test]
    fn create_dir_world_readable_grants_users_read_inherited_never_write() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("deployment-logs");
        create_dir_world_readable(&dir).unwrap();

        let aces = users_allow_aces(&dir);
        assert!(
            aces.iter()
                .any(|&(m, f)| m & READ_EXECUTE == READ_EXECUTE && f & OI_CI == OI_CI),
            "expected an inheritable Users ReadAndExecute ACE, got {aces:?}"
        );
        assert!(
            aces.iter().all(|&(m, _)| m & ANY_WRITE == 0),
            "Users must not get write: {aces:?}"
        );

        // Files and subdirectories created inside inherit the read ACE.
        let file = dir.join("codedeploy-agent-deployments.log");
        std::fs::write(&file, b"x").unwrap();
        let sub = dir.join("d-123");
        std::fs::create_dir(&sub).unwrap();
        for p in [&file, &sub] {
            let aces = users_allow_aces(p);
            assert!(
                aces.iter().any(|&(m, _)| m & READ_EXECUTE == READ_EXECUTE),
                "{} should inherit Users read, got {aces:?}",
                p.display()
            );
            assert!(aces.iter().all(|&(m, _)| m & ANY_WRITE == 0));
        }
    }

    #[test]
    fn create_dir_world_readable_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ongoing-deployment");
        create_dir_world_readable(&dir).unwrap();
        let first = users_allow_aces(&dir);
        create_dir_world_readable(&dir).unwrap();
        assert_eq!(users_allow_aces(&dir), first, "re-touching must not add ACEs");
    }

    #[test]
    fn create_dir_world_readable_adds_read_to_protected_dir() {
        // A dir a hardened agent left protected gains Users read and stays
        // protected (no inherited ACEs come back).
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("state");
        create_dir_secure(&dir, 0o700).unwrap();
        create_dir_world_readable(&dir).unwrap();

        let aces = users_allow_aces(&dir);
        assert!(aces.iter().any(|&(m, _)| m & READ_EXECUTE == READ_EXECUTE), "got {aces:?}");
        assert!(aces.iter().all(|&(m, _)| m & ANY_WRITE == 0));
    }

    #[test]
    fn create_root_dir_parity_adds_no_users_ace() {
        use windows_acl::acl::{ACL, AceType};

        // root_dir is the MSI install root on Windows: its ACL is the
        // installer's, so the parity default must not add an explicit ACE.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("CodeDeploy");
        create_root_dir(&root, 0o711, false).unwrap();

        let acl = ACL::from_file_path(root.to_str().unwrap(), false).unwrap();
        let explicit_users = acl
            .all()
            .unwrap()
            .iter()
            .filter(|e| {
                e.entry_type == AceType::AccessAllow
                    && e.string_sid == USERS_SID
                    && e.flags & INHERITED_ACE == 0
            })
            .count();
        assert_eq!(explicit_users, 0, "create_root_dir must leave the root ACL alone");
    }

    #[test]
    fn create_root_dir_restrict_applies_agent_dacl() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("CodeDeploy");
        create_root_dir(&root, 0o711, true).unwrap();
        assert_agent_dacl(&root);
    }
}
