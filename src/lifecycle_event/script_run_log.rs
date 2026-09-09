//! Synchronized script execution log.
//!
//! Wraps a log file and a bounded buffer behind a single `write_line` method.
//! Shared via `Arc<Mutex<ScriptRunLog>>` between the stdout/stderr stream tasks
//! in [`Script`](super::script::Script).
//!
//! Disk size is bounded by size-based rotation: 64 MiB per file, 8 files
//! retained (worst-case 512 MiB per deployment). Mirrors
//! `logging::deployment_logger::DeploymentLogger` so both per-deployment log
//! files share rotation semantics. The size cap prevents a runaway script
//! from filling the disk.

use super::bounded_fifo_vec::BoundedFifoVec;
use chrono::Local;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// Script run log mode for the given policy: 0644 by default (customers debug
/// `scripts.log` as non-root, log collectors ship it), 0640 under the opt-in
/// `restrict_agent_dir_permissions` hardening.
/// Matches `logging::deployment_logger::log_file_mode`.
#[cfg(unix)]
fn log_file_mode(restrict: bool) -> u32 {
    if restrict { 0o640 } else { 0o644 }
}

/// Maximum size of a single `scripts.log` file before rotation. 64 MiB.
/// Matches `logging::deployment_logger::MAX_FILE_SIZE`.
const MAX_FILE_SIZE: u64 = 64 * 1024 * 1024;

/// Maximum number of `scripts.log` files retained (current + 7 historical).
/// Worst-case disk footprint per deployment: `MAX_FILE_SIZE * MAX_FILES` = 512 MiB.
const MAX_FILES: usize = 8;

#[derive(Debug)]
pub struct ScriptRunLog {
    /// Path to the current log file. `None` for in-memory-only logs.
    path: Option<PathBuf>,
    file: Option<File>,
    buffer: BoundedFifoVec,
    /// Mode policy for the file (and rotation reopens); see [`log_file_mode`].
    restrict_permissions: bool,
}

impl ScriptRunLog {
    /// Open (or create) the log file in append mode with the default modes
    /// (`logs/` dir 0755, file 0644).
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or parent dirs cannot be created.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        Self::open_with_policy(path, false)
    }

    /// Open (or create) the log file in append mode.
    ///
    /// `restrict_permissions` mirrors the `restrict_agent_dir_permissions`
    /// config flag: `false` (default) gives world-readable 0755 dir / 0644
    /// file; `true` gives hardened 0750/0640.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or parent dirs cannot be created.
    pub fn open_with_policy(path: &Path, restrict_permissions: bool) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            crate::system::create_deployment_dir(parent, 0o750, restrict_permissions)?;
        }
        let file = open_log_file(path, restrict_permissions)?;
        Ok(Self {
            path: Some(path.to_path_buf()),
            file: Some(file),
            buffer: BoundedFifoVec::new(),
            restrict_permissions,
        })
    }

    /// Create an in-memory-only log (no file on disk).
    /// Used as fallback when the log file cannot be created.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            path: None,
            file: None,
            buffer: BoundedFifoVec::new(),
            restrict_permissions: false,
        }
    }

    /// Write a single line to both the file and the bounded buffer.
    ///
    /// `prefix` is `"[stdout]"`, `"[stderr]"`, or `""` for headers.
    /// Triggers rotation when the file reaches `MAX_FILE_SIZE`. Rotation
    /// errors are best-effort — losing log lines is preferable to failing
    /// a deployment.
    ///
    /// ANSI escape sequences are stripped from `line` before formatting so
    /// hostile script output cannot forge log entries via cursor positioning,
    /// screen clearing, or color codes.
    pub fn write_line(&mut self, prefix: &str, line: &str) {
        let line = strip_ansi_escapes::strip_str(line);
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S");
        let formatted = format!("{ts} {prefix}{line}\n");

        // Rotate before write so the new line lands in the fresh file.
        let _ = self.rotate_if_needed();

        if let Some(ref mut file) = self.file {
            let _ = file.write_all(formatted.as_bytes());
            let _ = file.flush();
        }
        self.buffer.push(formatted);
    }

    /// Rotate the log file if it has reached the size cap.
    /// `.7 → delete, .6 → .7, ... .1 → .2, current → .1`. Mirrors
    /// `DeploymentLogger::rotate_if_needed` byte-for-byte.
    fn rotate_if_needed(&mut self) -> std::io::Result<()> {
        let (Some(path), Some(file)) = (self.path.as_ref(), self.file.as_ref()) else {
            return Ok(());
        };
        let size = file.metadata()?.len();
        if size < MAX_FILE_SIZE {
            return Ok(());
        }

        for i in (1..MAX_FILES).rev() {
            let from = rotated_path(path, i);
            let to = rotated_path(path, i + 1);
            if from.exists() {
                if i + 1 >= MAX_FILES {
                    fs::remove_file(&from)?;
                } else {
                    fs::rename(&from, &to)?;
                }
            }
        }

        let first_rotated = rotated_path(path, 1);
        fs::rename(path, &first_rotated)?;

        match open_log_file(path, self.restrict_permissions) {
            Ok(f) => self.file = Some(f),
            Err(e) => {
                self.file = None;
                return Err(e);
            },
        }

        Ok(())
    }

    /// Get a copy of the buffered entries for diagnostics.
    #[must_use]
    pub fn entries(&self) -> Vec<String> {
        self.buffer.to_vec()
    }

    /// Consume the log and return the buffered entries for diagnostics.
    #[must_use]
    pub fn into_entries(self) -> Vec<String> {
        self.buffer.into_vec()
    }
}

/// Open (or create) the script log file with the policy mode.
/// Unix: 0644 default, 0640 restricted; force-set on every
/// open so a file a tightened agent left behind heals on the next event.
/// Other platforms: platform defaults.
fn open_log_file(path: &Path, restrict: bool) -> std::io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = log_file_mode(restrict);
        let f = OpenOptions::new().create(true).append(true).mode(mode).open(path)?;
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        Ok(f)
    }
    #[cfg(not(unix))]
    {
        let _ = restrict;
        OpenOptions::new().create(true).append(true).open(path)
    }
}

fn rotated_path(base: &Path, index: usize) -> PathBuf {
    let name = base
        .file_name()
        .unwrap_or_else(|| std::ffi::OsStr::new("scripts.log"))
        .to_string_lossy();
    base.with_file_name(format!("{name}.{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Grow the file past the rotation cap through a dedicated write
    /// handle (sparse; nothing is actually written).
    fn inflate_past_cap(path: &Path) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_len(MAX_FILE_SIZE + 1)
            .unwrap();
    }

    #[test]
    fn open_creates_parent_dirs_and_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sub/dir/log.txt");

        let log = ScriptRunLog::open(&path).unwrap();
        assert!(path.exists());
        assert!(log.entries().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn open_default_creates_world_readable_dir_and_file() {
        // Customers tail scripts.log as non-root while debugging hooks, so the
        // default modes must stay 0755 dir / 0644 file.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("logs/scripts.log");

        let _log = ScriptRunLog::open(&path).unwrap();

        let dir_mode = fs::metadata(dir.path().join("logs")).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o755, "logs dir mode {dir_mode:#o}, want 0755");
        assert_eq!(file_mode, 0o644, "scripts.log mode {file_mode:#o}, want 0644");
    }

    #[cfg(unix)]
    #[test]
    fn open_restricted_creates_hardened_dir_and_file() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("logs/scripts.log");

        let _log = ScriptRunLog::open_with_policy(&path, true).unwrap();

        let dir_mode = fs::metadata(dir.path().join("logs")).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o750, "logs dir mode {dir_mode:#o}, want 0750");
        assert_eq!(file_mode, 0o640, "scripts.log mode {file_mode:#o}, want 0640");
    }

    #[cfg(unix)]
    #[test]
    fn open_default_loosens_file_tightened_by_previous_agent() {
        // Upgrade path: heal a 0640 scripts.log left by a tightened agent.
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let logs = dir.path().join("logs");
        fs::create_dir_all(&logs).unwrap();
        let path = logs.join("scripts.log");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&logs, fs::Permissions::from_mode(0o750)).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let _log = ScriptRunLog::open(&path).unwrap();

        let dir_mode = fs::metadata(&logs).unwrap().permissions().mode() & 0o777;
        let file_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o755, "expected 0750 -> 0755 heal, got {dir_mode:#o}");
        assert_eq!(file_mode, 0o644, "expected 0640 -> 0644 heal, got {file_mode:#o}");
    }

    #[test]
    fn in_memory_has_no_file() {
        let log = ScriptRunLog::in_memory();
        assert!(log.entries().is_empty());
    }

    #[test]
    fn write_line_appends_to_buffer_and_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("log.txt");

        let mut log = ScriptRunLog::open(&path).unwrap();
        log.write_line("[stdout]", "hello world");

        let entries = log.entries();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].contains("[stdout]hello world"));

        let file_content = std::fs::read_to_string(&path).unwrap();
        assert!(file_content.contains("[stdout]hello world"));
    }

    #[test]
    fn write_line_in_memory_only_buffers() {
        let mut log = ScriptRunLog::in_memory();
        log.write_line("[stderr]", "error msg");

        let entries = log.entries();
        assert_eq!(entries.len(), 1);
        assert!(entries[0].contains("[stderr]error msg"));
    }

    #[test]
    fn into_entries_consumes_log() {
        let mut log = ScriptRunLog::in_memory();
        log.write_line("", "line1");
        log.write_line("", "line2");

        let entries = log.into_entries();
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn rotated_path_appends_index() {
        use std::path::PathBuf;
        let base = PathBuf::from("/var/log/scripts.log");
        assert_eq!(rotated_path(&base, 1), PathBuf::from("/var/log/scripts.log.1"));
        assert_eq!(rotated_path(&base, 7), PathBuf::from("/var/log/scripts.log.7"));
    }

    #[test]
    fn rotation_skipped_when_under_cap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scripts.log");
        let mut log = ScriptRunLog::open(&path).unwrap();
        log.write_line("[stdout]", "small message");
        // No rotation should have happened — only the live file exists.
        assert!(path.exists());
        assert!(!rotated_path(&path, 1).exists());
    }

    #[test]
    fn rotation_triggered_when_file_exceeds_cap() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scripts.log");
        let mut log = ScriptRunLog::open(&path).unwrap();
        log.write_line("[stdout]", "before rotation");

        // Inflate live file past MAX_FILE_SIZE (sparse — set_len doesn't
        // actually write 64 MiB). Uses a separate write handle: on Windows
        // the log's append-mode handle lacks FILE_WRITE_DATA, so set_len
        // on it is denied.
        inflate_past_cap(&path);

        log.write_line("[stdout]", "after rotation");

        let rotated = rotated_path(&path, 1);
        assert!(rotated.exists(), "rotated file .1 should exist");
        assert!(path.exists(), "current file should be re-created");

        let live_content = std::fs::read_to_string(&path).unwrap();
        assert!(
            live_content.contains("after rotation"),
            "live file should contain the post-rotation line, got: {live_content:?}"
        );
        assert!(
            !live_content.contains("before rotation"),
            "live file should NOT contain pre-rotation content"
        );
    }

    #[test]
    fn rotation_drops_oldest_when_at_max_files() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("scripts.log");
        let mut log = ScriptRunLog::open(&path).unwrap();

        // Pre-create .1..=.7 so the next rotation must drop the oldest.
        for i in 1..MAX_FILES {
            let rp = rotated_path(&path, i);
            std::fs::write(&rp, format!("old-{i}")).unwrap();
        }

        inflate_past_cap(&path);
        log.write_line("[stdout]", "newest");

        // After rotation: old .6 → .7, old .7 dropped, .1 holds the rotated live file.
        let content_7 = std::fs::read_to_string(rotated_path(&path, 7)).unwrap();
        assert_eq!(content_7, "old-6", "old .6 should have moved to .7 (old .7 dropped)");
        assert!(rotated_path(&path, 1).exists(), ".1 should hold the rotated live file");
    }

    #[test]
    fn worst_case_disk_footprint_per_deployment_is_512_mib() {
        // Capacity invariant: MAX_FILES files × MAX_FILE_SIZE bytes.
        const ONE_MIB: u64 = 1024 * 1024;
        let total = (MAX_FILES as u64) * MAX_FILE_SIZE;
        assert_eq!(total, 512 * ONE_MIB, "expected 512 MiB worst-case ceiling");
    }
}
