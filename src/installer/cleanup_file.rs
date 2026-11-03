//! Cleanup file line encoding.
//!
//! Each line is a path. On Unix the path's raw bytes are written so names that
//! are not valid UTF-8 round-trip exactly. On Windows paths are written as text.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Prefix of a line that names an `SELinux` context to remove.
pub const SEMANAGE_PREFIX: &[u8] = b"semanage\0";

/// Write `prefix` followed by `path` and a newline.
///
/// # Errors
/// Returns `InvalidInput` if `path` contains a line break, or any error from
/// the underlying writer.
pub fn write_line(out: &mut dyn Write, prefix: &[u8], path: &Path) -> io::Result<()> {
    let bytes = path_to_bytes(path);
    if bytes.iter().any(|b| matches!(b, b'\n' | b'\r')) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path contains a line break: {}", path.display().to_string().escape_debug()),
        ));
    }
    let mut line = prefix.to_vec();
    line.extend_from_slice(&bytes);
    line.push(b'\n');
    out.write_all(&line)
}

#[cfg(unix)]
fn path_to_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_to_bytes(path: &Path) -> Vec<u8> {
    path.display().to_string().into_bytes()
}

/// Build a path from the bytes of one cleanup line.
#[cfg(unix)]
#[must_use]
pub fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// Build a path from the bytes of one cleanup line.
#[cfg(not(unix))]
#[must_use]
pub fn path_from_bytes(bytes: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_line_rejects_line_breaks() {
        for path in ["/opt/app/a\nb", "/opt/app/a\rb"] {
            let mut buf = Vec::new();
            let err = write_line(&mut buf, b"", Path::new(path)).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
            assert!(buf.is_empty());
        }
    }

    #[test]
    fn round_trips_plain_path() {
        let mut buf = Vec::new();
        write_line(&mut buf, b"", Path::new("/opt/app/a.txt")).unwrap();
        assert_eq!(buf, b"/opt/app/a.txt\n");
        assert_eq!(path_from_bytes(&buf[..buf.len() - 1]), PathBuf::from("/opt/app/a.txt"));
    }

    #[test]
    fn writes_prefix() {
        let mut buf = Vec::new();
        write_line(&mut buf, SEMANAGE_PREFIX, Path::new("/opt/app")).unwrap();
        assert_eq!(buf, b"semanage\0/opt/app\n");
    }

    #[cfg(unix)]
    #[test]
    fn round_trips_non_utf8_path() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = Path::new(OsStr::from_bytes(b"/opt/caf\xe9.txt"));
        let mut buf = Vec::new();
        write_line(&mut buf, b"", path).unwrap();
        assert_eq!(buf, b"/opt/caf\xe9.txt\n");
        assert_eq!(path_from_bytes(&buf[..buf.len() - 1]), path);
    }
}
