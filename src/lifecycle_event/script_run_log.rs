//! @risk none
//!
//! Synchronized script execution log.
//!
//! Wraps a log file and a bounded buffer behind a single `write_line` method.
//! Shared via `Arc<Mutex<ScriptRunLog>>` between the stdout/stderr stream tasks
//! in [`Script`](super::script::Script).

use super::bounded_fifo_vec::BoundedFifoVec;
use chrono::Local;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

#[derive(Debug)]
pub struct ScriptRunLog {
    file: Option<File>,
    buffer: BoundedFifoVec,
}

impl ScriptRunLog {
    /// Open (or create) the log file in append mode.
    ///
    /// # Errors
    /// Returns an error if the file cannot be opened or parent dirs cannot be created.
    pub fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self { file: Some(file), buffer: BoundedFifoVec::new() })
    }

    /// Create an in-memory-only log (no file on disk).
    /// Used as fallback when the log file cannot be created.
    #[must_use]
    pub fn in_memory() -> Self {
        Self { file: None, buffer: BoundedFifoVec::new() }
    }

    /// Write a single line to both the file and the bounded buffer.
    ///
    /// `prefix` is `"[stdout]"`, `"[stderr]"`, or `""` for headers.
    pub fn write_line(&mut self, prefix: &str, line: &str) {
        let ts = Local::now().format("%Y-%m-%d %H:%M:%S");
        let formatted = format!("{ts} {prefix}{line}\n");
        if let Some(ref mut file) = self.file {
            let _ = file.write_all(formatted.as_bytes());
            let _ = file.flush();
        }
        self.buffer.push(formatted);
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn open_creates_parent_dirs_and_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("sub/dir/log.txt");

        let log = ScriptRunLog::open(&path).unwrap();
        assert!(path.exists());
        assert!(log.entries().is_empty());
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
}
