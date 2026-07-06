//! @risk low
//!
//! Agent version-tracking file (`.version`).
//!
//! The `.version` file is part of the agent's documented install layout: it
//! sits next to the install root, is written at startup, and is world-readable
//! read-only (mode 0444) so host tooling can read the installed version.
//!
//! The body carries the bare version (`agent_version: <version>`), matching the
//! `CARGO_PKG_VERSION` the agent reports on the
//! `x-amz-codedeploy-agent-version` header.

use crate::system::secure_files::write_file_secure;
use std::io;
use std::path::Path;

/// The agent version reported by both the `.version` file and the
/// `x-amz-codedeploy-agent-version` header (which aliases this constant), so
/// the two can never disagree.
pub const AGENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// `.version` file mode on Unix: world-readable read-only `0444`.
/// Ignored on Windows, where the secure writer applies a protected DACL.
const VERSION_FILE_MODE: u32 = 0o444;

/// Render the `.version` file body: `agent_version: <version>`.
#[must_use]
pub fn version_file_contents(version: &str) -> String {
    format!("agent_version: {version}")
}

/// Write the agent `.version` file to `path`. Only the file is secured
/// (`write_file_secure`); the parent is `mkdir -p`'d but never re-secured — it
/// is the shared install root that host tooling must keep reading.
///
/// # Errors
/// Returns an error if the parent directory cannot be created or the file
/// cannot be written.
pub fn write_version_file(path: &Path) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_file_secure(path, version_file_contents(AGENT_VERSION).as_bytes(), VERSION_FILE_MODE)
}

/// Write the `.version` file to its canonical path ([`crate::paths::version_file`]).
/// The single entry point both startup paths (Unix master, Windows service) call.
///
/// # Errors
/// Returns an error if the parent directory cannot be created or the file
/// cannot be written.
pub fn write() -> io::Result<()> {
    write_version_file(&crate::paths::version_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_are_bare_version() {
        assert_eq!(version_file_contents("2.0.0"), "agent_version: 2.0.0");
    }

    #[test]
    fn contents_parse_as_version_header_value() {
        // Readers take everything after `": "` as the header value.
        let body = version_file_contents(AGENT_VERSION);
        let header_value = body.split(": ").last().unwrap().trim();
        assert_eq!(header_value, AGENT_VERSION);
    }

    #[test]
    fn write_then_read_roundtrips() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested").join(".version");
        write_version_file(&path).unwrap();
        let read = std::fs::read_to_string(&path).unwrap();
        assert_eq!(read, version_file_contents(AGENT_VERSION));
        assert_eq!(read, format!("agent_version: {AGENT_VERSION}"));
    }

    /// On Unix the file must land at `0444`, umask-independent.
    #[cfg(unix)]
    #[test]
    fn write_version_file_applies_secure_readonly_mode() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested").join(".version");
        write_version_file(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o444, "expected 0444 (world-readable read-only), got {mode:#o}");
    }
}
