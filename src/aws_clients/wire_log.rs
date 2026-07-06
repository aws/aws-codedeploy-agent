//! @risk medium
//!
//! Amazon S3 HTTP wire logging (`:log_aws_wire:`).
//!
//! When the `log_aws_wire` config setting is enabled, [`WireLogInterceptor`] is
//! attached to the S3 client and appends every request/response (method, URI,
//! status, and headers) to a dedicated, size-rotated log file
//! (`<program_name>.aws_wire.log`) in the agent log directory.
//!
//! The filename `<program_name>.aws_wire.log` and the ~1 GB budget
//! (16 × 64 MB) are the publicly documented behavior of this setting, so both
//! are kept as-is.
//!
//! SECURITY: the published docs warn the wire log "might contain sensitive
//! information, including the plain-text contents of files transferred into, or
//! out of, Amazon S3." Unlike the world-readable agent log, this file is created
//! `0640` (owner read/write, group read, no other access) so it is not exposed
//! to unprivileged users. Header values themselves are written verbatim, as a
//! wire trace is expected to; `Authorization` is redacted defensively (see
//! `redact_header`).

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

// All interceptor types are reached through the `aws_sdk_s3` re-exports so we do
// not need a direct dependency on `aws-smithy-runtime-api`.
use aws_sdk_s3::config::ConfigBag;
use aws_sdk_s3::config::Intercept;
use aws_sdk_s3::config::RuntimeComponents;
use aws_sdk_s3::config::interceptors::{
    AfterDeserializationInterceptorContextRef, BeforeTransmitInterceptorContextRef,
};
use aws_sdk_s3::error::BoxError;

/// Maximum size of the active wire log before it rotates, in bytes (64 MiB).
const MAX_WIRE_LOG_SIZE: u64 = 64 * 1024 * 1024;

/// Number of rotated wire-log chunks kept (in addition to the active file).
/// 16 × 64 MB ≈ 1 GB of history.
const WIRE_LOG_KEEP: usize = 16;

/// Restrictive mode for the wire log: owner rw, group r, no other access.
/// The wire log can contain sensitive payload data, so it is NOT world-readable
/// (unlike the agent's own operational log).
#[cfg(unix)]
const WIRE_LOG_MODE: u32 = 0o640;

/// Builds the wire-log path from the log directory and program name:
/// `<log_dir>/<program_name>.aws_wire.log`, the documented wire-log filename.
#[must_use]
pub fn wire_log_path(log_dir: &Path, program_name: &str) -> PathBuf {
    log_dir.join(format!("{program_name}.aws_wire.log"))
}

/// A size-rotating append writer for the wire log.
///
/// Rotation is count-based (`<name>.aws_wire.log.1` … `.N`) rather than the
/// agent log's date-stamped scheme, because wire logs are short-lived debugging
/// artifacts keyed by size, not day.
#[derive(Debug)]
struct WireLogWriter {
    path: PathBuf,
    file: File,
    size: u64,
}

impl WireLogWriter {
    fn new(path: PathBuf) -> io::Result<Self> {
        let file = open_wire_log(&path)?;
        let size = file.metadata()?.len();
        Ok(Self { path, file, size })
    }

    /// Shift `.N-1 → .N`, drop the oldest, move the active file to `.1`, and open
    /// a fresh active file. Best-effort: a rename failure leaves the current file
    /// in place and logging continues.
    fn rotate(&mut self) -> io::Result<()> {
        // Drop the oldest chunk.
        let oldest = self.chunk_path(WIRE_LOG_KEEP);
        let _ = std::fs::remove_file(&oldest);
        // Shift .N-1 -> .N down to .1 -> .2.
        for n in (1..WIRE_LOG_KEEP).rev() {
            let from = self.chunk_path(n);
            let to = self.chunk_path(n + 1);
            if from.exists() {
                let _ = std::fs::rename(&from, &to);
            }
        }
        // Active -> .1, then reopen a fresh active file.
        let _ = std::fs::rename(&self.path, self.chunk_path(1));
        self.file = open_wire_log(&self.path)?;
        self.size = 0;
        Ok(())
    }

    fn chunk_path(&self, n: usize) -> PathBuf {
        let mut p = self.path.clone().into_os_string();
        p.push(format!(".{n}"));
        PathBuf::from(p)
    }
}

impl Write for WireLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.size >= MAX_WIRE_LOG_SIZE {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Open (or create) the wire log in append mode with restrictive perms.
fn open_wire_log(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let f = OpenOptions::new().create(true).append(true).mode(WIRE_LOG_MODE).open(path)?;
        // Re-apply in case the file pre-existed with a wider mode.
        f.set_permissions(std::fs::Permissions::from_mode(WIRE_LOG_MODE))?;
        Ok(f)
    }
    #[cfg(not(unix))]
    {
        OpenOptions::new().create(true).append(true).open(path)
    }
}

/// Redact header values that must never be persisted, even in a restricted wire
/// log. `Authorization` carries the `SigV4` signature; while short-lived, there is
/// no debugging value in logging it verbatim.
fn redact_header(name: &str, value: &str) -> String {
    if name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case("x-amz-security-token")
    {
        "<redacted>".to_string()
    } else {
        value.to_string()
    }
}

/// SDK interceptor that appends S3 request/response wire traces to the wire log.
///
/// Registered on the S3 client only when `log_aws_wire` is enabled. Holds the
/// rotating writer behind a `Mutex` because interceptor hooks may run
/// concurrently across the SDK's async tasks. A poisoned lock or write error is
/// swallowed (best-effort logging must never fail a deployment).
#[derive(Debug)]
pub struct WireLogInterceptor {
    writer: Mutex<WireLogWriter>,
}

impl WireLogInterceptor {
    /// Create an interceptor writing to `<log_dir>/<program_name>.aws_wire.log`.
    ///
    /// # Errors
    /// Returns an error if the wire log file cannot be opened/created.
    pub fn new(log_dir: &Path, program_name: &str) -> io::Result<Self> {
        let path = wire_log_path(log_dir, program_name);
        Ok(Self { writer: Mutex::new(WireLogWriter::new(path)?) })
    }

    fn append(&self, line: &str) {
        if let Ok(mut w) = self.writer.lock() {
            let _ = w.write_all(line.as_bytes());
            let _ = w.flush();
        }
    }
}

impl Intercept for WireLogInterceptor {
    fn name(&self) -> &'static str {
        "CodeDeployWireLog"
    }

    fn read_before_transmit(
        &self,
        context: &BeforeTransmitInterceptorContextRef<'_>,
        _rc: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        use std::fmt::Write as _;
        let req = context.request();
        let mut line = format!("opening connection: {} {}\n", req.method(), req.uri());
        for (name, value) in req.headers() {
            let _ = writeln!(line, "-> {name}: {}", redact_header(name, value));
        }
        self.append(&line);
        Ok(())
    }

    fn read_after_deserialization(
        &self,
        context: &AfterDeserializationInterceptorContextRef<'_>,
        _rc: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        use std::fmt::Write as _;
        let resp = context.response();
        let mut line = format!("<- response status: {}\n", resp.status());
        for (name, value) in resp.headers() {
            let _ = writeln!(line, "<- {name}: {}", redact_header(name, value));
        }
        self.append(&line);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_log_path_matches_documented_filename() {
        let p = wire_log_path(Path::new("/var/log/aws/codedeploy-agent"), "codedeploy-agent");
        assert_eq!(p, PathBuf::from("/var/log/aws/codedeploy-agent/codedeploy-agent.aws_wire.log"));
    }

    #[test]
    fn wire_log_path_honors_program_name() {
        let p = wire_log_path(Path::new("/logs"), "codedeploy-agent-rust");
        assert_eq!(p, PathBuf::from("/logs/codedeploy-agent-rust.aws_wire.log"));
    }

    #[test]
    fn redact_hides_authorization_and_token() {
        assert_eq!(redact_header("Authorization", "AWS4-HMAC-SHA256 ..."), "<redacted>");
        assert_eq!(redact_header("authorization", "sig"), "<redacted>");
        assert_eq!(redact_header("X-Amz-Security-Token", "tok"), "<redacted>");
        assert_eq!(redact_header("Content-Length", "42"), "42");
    }

    #[cfg(unix)]
    #[test]
    fn wire_log_created_restricted_0640() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = wire_log_path(dir.path(), "codedeploy-agent");
        let _w = WireLogWriter::new(path.clone()).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "wire log must be 0640 (not world-readable), got {mode:#o}");
    }

    #[test]
    fn writer_rotates_at_size_cap_and_keeps_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let path = wire_log_path(dir.path(), "codedeploy-agent");
        let mut w = WireLogWriter::new(path.clone()).unwrap();

        // Force the size trigger, then write to rotate.
        w.size = MAX_WIRE_LOG_SIZE;
        w.write_all(b"after rotation\n").unwrap();

        // Active file present with the new content; .1 chunk exists.
        assert!(path.exists());
        assert!(std::fs::read_to_string(&path).unwrap().contains("after rotation"));
        assert!(w.chunk_path(1).exists(), "first rotated chunk must exist");
    }

    #[test]
    fn interceptor_new_creates_file_and_appends() {
        let dir = tempfile::tempdir().unwrap();
        let ixn = WireLogInterceptor::new(dir.path(), "codedeploy-agent").unwrap();
        ixn.append("hello wire\n");
        let content =
            std::fs::read_to_string(wire_log_path(dir.path(), "codedeploy-agent")).unwrap();
        assert!(content.contains("hello wire"));
    }

    #[test]
    fn interceptor_reports_stable_name() {
        let dir = tempfile::tempdir().unwrap();
        let ixn = WireLogInterceptor::new(dir.path(), "codedeploy-agent").unwrap();
        assert_eq!(ixn.name(), "CodeDeployWireLog");
    }
}
