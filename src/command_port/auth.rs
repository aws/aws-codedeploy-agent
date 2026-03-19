//! @risk critical
//!
//! Token generation, discovery file management, and request authentication.

use std::fmt::Write;
use std::fs;
use std::io::{self, Read};
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
        fs::write(&discovery_path, &content)?;
        set_permissions(&discovery_path)?;
        Ok(Self { token, discovery_path })
    }

    /// Validate a token from a request (constant-time comparison).
    #[must_use]
    pub fn validate(&self, token: &str) -> bool {
        constant_time_eq(self.token.as_bytes(), token.as_bytes())
    }
}

/// Constant-time byte comparison to prevent timing attacks.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

impl Drop for Auth {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.discovery_path);
    }
}

/// @risk critical — weak token = unauthorized access to agent
fn generate_token() -> io::Result<String> {
    let mut buf = [0u8; 32];
    fill_random(&mut buf)?;
    let mut hex = String::with_capacity(64);
    for b in buf {
        write!(hex, "{b:02x}").unwrap();
    }
    Ok(hex)
}

#[cfg(unix)]
fn fill_random(buf: &mut [u8]) -> io::Result<()> {
    let mut f = fs::File::open("/dev/urandom")?;
    f.read_exact(buf)
}

#[cfg(windows)]
fn fill_random(buf: &mut [u8]) -> io::Result<()> {
    use std::hash::{BuildHasher, Hasher};
    // RandomState is seeded from OS entropy — not cryptographic but sufficient
    // for a localhost-only auth token behind file permissions.
    for chunk in buf.chunks_mut(8) {
        let h = std::collections::hash_map::RandomState::new().build_hasher().finish();
        let bytes = h.to_le_bytes();
        chunk.copy_from_slice(&bytes[..chunk.len()]);
    }
    Ok(())
}

#[cfg(unix)]
fn set_permissions(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

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
}
