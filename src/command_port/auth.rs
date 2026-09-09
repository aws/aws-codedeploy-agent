//! Token generation, discovery file management, and request authentication.

use std::fmt::Write;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Auth token and port discovery for the command port.
#[derive(Debug)]
pub struct Auth {
    token: String,
    discovery_path: PathBuf,
}

impl Auth {
    /// Generate a new random token and write the discovery file.
    ///
    /// # Errors
    /// Returns an error if the discovery file cannot be written.
    pub fn init(discovery_path: PathBuf, port: u16) -> io::Result<Self> {
        let token = generate_token()?;
        let content = serde_json::json!({"port": port, "token": token}).to_string();
        if let Some(parent) = discovery_path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_discovery_file(&discovery_path, content.as_bytes())?;
        Ok(Self { token, discovery_path })
    }

    /// Validate a token from a request (constant-time comparison).
    #[must_use]
    pub fn validate(&self, token: &str) -> bool {
        constant_time_eq(self.token.as_bytes(), token.as_bytes())
    }
}

/// Constant-time byte comparison to prevent timing attacks.
/// Uses the `subtle` crate which is audited and handles length differences
/// without leaking length information via timing.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

impl Drop for Auth {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.discovery_path);
    }
}

/// A weak token allows unauthorized access to the agent.
fn generate_token() -> io::Result<String> {
    let mut buf = [0u8; 32];
    fill_random(&mut buf)?;
    let mut hex = String::with_capacity(64);
    for b in buf {
        // fmt::Write for String is infallible — allocation happened in with_capacity.
        write!(hex, "{b:02x}").expect("String write is infallible");
    }
    Ok(hex)
}

fn fill_random(buf: &mut [u8]) -> io::Result<()> {
    getrandom::getrandom(buf).map_err(io::Error::other)
}

/// Full control access mask for Windows DACL entries.
#[cfg(all(windows, test))]
const GENERIC_ALL: u32 = 0x1000_0000;

/// Write the discovery file with restricted permissions atomically (no TOCTOU).
/// Removes any pre-existing file first and uses `O_EXCL` so the mode is always
/// applied on creation. If an attacker pre-creates the file between remove and
/// open, `create_new` fails with `AlreadyExists` — a clean error, not a leak.
#[cfg(unix)]
fn write_discovery_file(path: &Path, content: &[u8]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let _ = fs::remove_file(path);
    let mut file = fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    file.write_all(content)
}

/// Write the discovery file atomically with a restricted DACL (no TOCTOU).
///
/// Creates the file via `CreateFileW` with a pre-built `SECURITY_ATTRIBUTES`
/// containing an SDDL-derived security descriptor. The DACL is applied at
/// creation time so the file is never visible with inherited (permissive) ACLs.
///
/// SDDL `D:P(A;;GA;;;SY)(A;;GA;;;BA)`:
/// - `D:P` = protected DACL (blocks inheritance from parent directory)
/// - `(A;;GA;;;SY)` = Allow `GENERIC_ALL` to `NT AUTHORITY\SYSTEM`
/// - `(A;;GA;;;BA)` = Allow `GENERIC_ALL` to `BUILTIN\Administrators`
#[cfg(windows)]
#[allow(unsafe_code)]
fn write_discovery_file(path: &Path, content: &[u8]) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;

    use windows_sys::Win32::Foundation::{INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::ConvertStringSecurityDescriptorToSecurityDescriptorW;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_WRITE,
    };

    // Remove existing file so CREATE_NEW applies SECURITY_ATTRIBUTES to a fresh
    // file. CREATE_ALWAYS would silently preserve inherited (permissive) ACLs on
    // an existing file, ignoring lpSecurityDescriptor.
    match fs::remove_file(path) {
        Ok(()) => {},
        Err(e) if e.kind() == io::ErrorKind::NotFound => {},
        Err(e) => return Err(e),
    }

    let sddl: Vec<u16> = "D:P(A;;GA;;;SY)(A;;GA;;;BA)\0".encode_utf16().collect();

    let mut sd: *mut core::ffi::c_void = std::ptr::null_mut();
    // SAFETY: `sddl` is a NUL-terminated UTF-16 literal; `&mut sd` is a valid
    // out-pointer; `null_mut()` for the size parameter is documented as
    // "caller does not need the size returned".
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

    let sa = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0, // FALSE
    };

    let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();

    // SAFETY: `wide_path` is NUL-terminated; `&sa` lives for the duration of
    // the call; return value is checked against `INVALID_HANDLE_VALUE`.
    let handle = unsafe {
        CreateFileW(
            wide_path.as_ptr(),
            FILE_GENERIC_WRITE,
            0, // no sharing
            &sa,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(), // no template
        )
    };

    // Capture last-error before LocalFree can clobber it.
    let create_err = if handle == INVALID_HANDLE_VALUE {
        Some(io::Error::last_os_error())
    } else {
        None
    };

    // SD only needs to live until CreateFileW returns — free immediately.
    // SAFETY: `sd` was allocated by `ConvertStringSecurityDescriptorToSecurityDescriptorW`
    // on the success path and must be freed exactly once.
    unsafe { LocalFree(sd) };

    if let Some(err) = create_err {
        return Err(err);
    }

    // SAFETY: `handle` was just validated (not INVALID_HANDLE_VALUE) and no
    // other reference to it exists. Ownership transfers to `File`.
    let mut file = unsafe { std::fs::File::from_raw_handle(handle as *mut core::ffi::c_void) };
    std::io::Write::write_all(&mut file, content)
}

#[cfg(not(any(unix, windows)))]
compile_error!("command port discovery file requires Unix or Windows for secure file permissions");

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_discovery_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state/.command-port");
        let auth = Auth::init(path.clone(), 12345).unwrap();

        let content = fs::read_to_string(&path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["port"], 12345);
        assert!(!parsed["token"].as_str().unwrap().is_empty());
        assert!(auth.validate(parsed["token"].as_str().unwrap()));
    }

    #[test]
    fn validate_correct_token() {
        let dir = TempDir::new().unwrap();
        let auth = Auth::init(dir.path().join(".cp"), 1).unwrap();
        let content = fs::read_to_string(dir.path().join(".cp")).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(auth.validate(parsed["token"].as_str().unwrap()));
    }

    #[test]
    fn validate_wrong_token() {
        let dir = TempDir::new().unwrap();
        let auth = Auth::init(dir.path().join(".cp"), 1).unwrap();
        assert!(!auth.validate("wrong"));
    }

    #[test]
    fn drop_removes_discovery_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".cp");
        {
            let _auth = Auth::init(path.clone(), 1).unwrap();
            assert!(path.exists());
        }
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn discovery_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".cp");
        let _auth = Auth::init(path.clone(), 1).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn token_is_64_hex_chars() {
        let token = generate_token().unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn token_is_unique() {
        assert_ne!(generate_token().unwrap(), generate_token().unwrap());
    }

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq(b"abc", b"abc"));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq(b"abc", b"xyz"));
    }

    #[test]
    fn constant_time_eq_different_length() {
        assert!(!constant_time_eq(b"abc", b"ab"));
    }

    #[cfg(windows)]
    #[test]
    fn discovery_file_dacl_restricts_to_system_and_admins() {
        use windows_acl::acl::{ACL, AceType};

        let dir = TempDir::new().unwrap();
        let path = dir.path().join(".cp");
        let _auth = Auth::init(path.clone(), 1).unwrap();

        let acl = ACL::from_file_path(path.to_str().unwrap(), false).unwrap();
        let entries = acl.all().unwrap();

        let allow: Vec<_> =
            entries.iter().filter(|e| e.entry_type == AceType::AccessAllow).collect();
        assert_eq!(allow.len(), 2, "expected exactly 2 AccessAllow entries, got {}", allow.len());

        const INHERITED_ACE: u8 = 0x10;

        for e in &allow {
            assert_eq!(e.flags & INHERITED_ACE, 0, "entry {} is inherited", e.string_sid);
            const FILE_ALL_ACCESS: u32 = 0x001F_01FF;
            assert!(
                e.mask == GENERIC_ALL || e.mask == FILE_ALL_ACCESS,
                "entry {} has unexpected mask {:x}",
                e.string_sid,
                e.mask
            );
        }

        let sids: Vec<&str> = allow.iter().map(|e| e.string_sid.as_str()).collect();
        assert!(sids.contains(&"S-1-5-18"), "SYSTEM not in DACL: {sids:?}");
        assert!(sids.contains(&"S-1-5-32-544"), "Administrators not in DACL: {sids:?}");
    }
}
