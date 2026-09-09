//! Agent log configuration — file rotation and format setup.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use chrono::{Duration, Local, NaiveDate};
use time::macros::format_description;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::prelude::*;

use super::LogConfig;

/// Active log file is rotated once it reaches this size, in addition to the
/// daily boundary. Matches `deployment_logger`'s 64 MB convention.
const MAX_LOG_SIZE: u64 = 64 * 1024 * 1024;

/// Rotated log files older than this (by the date in their name) are pruned.
/// Matches the documented `CodeDeploy` behavior ("deleted after seven days").
const RETENTION_DAYS: i64 = 7;

/// Agent log file mode for the `restrict_log_dir_permissions` policy:
/// `0644` (world-readable) by default, `0640` under opt-in hardening.
///
/// The agent's own log (and the updater log, which shares this directory)
/// contain only operational diagnostics — no credentials, deployment payloads,
/// or customer data — so by default they are readable by non-root log
/// collectors (`CloudWatch` agent, fluentd, Datadog, …). The opt-in hardening
/// breaks exactly that; enable only where log shipping runs as root.
#[cfg(unix)]
fn log_file_mode(restrict: bool) -> u32 {
    if restrict { 0o640 } else { 0o644 }
}

/// Guard that keeps the non-blocking log writer alive.
/// Must be held for the lifetime of the process.
#[derive(Debug)]
pub struct LogGuard {
    _worker_guard: WorkerGuard,
}

fn build_level_filter(verbose: bool) -> EnvFilter {
    let level = if verbose { "debug" } else { "info" };
    EnvFilter::new(level)
}

fn build_timer() -> OffsetTime<&'static [time::format_description::BorrowedFormatItem<'static>]> {
    OffsetTime::new(
        time::UtcOffset::UTC,
        format_description!("[year]-[month]-[day] [hour]:[minute]:[second]"),
    )
}

/// A `Write` sink that keeps a stable active log file (`<prefix>.log`) and
/// rotates it to a dated archive (`<prefix>.YYYYMMDD.log`) when the day changes
/// or the active file exceeds [`MAX_LOG_SIZE`].
///
/// Owned exclusively by `tracing_appender`'s non-blocking worker thread, so it
/// needs no internal locking — writes arrive serially.
struct RotatingWriter {
    dir: PathBuf,
    prefix: String,
    active: PathBuf,
    file: File,
    size: u64,
    /// Local date the active file's content belongs to (set when opened).
    opened_day: NaiveDate,
    /// Mode policy from `restrict_log_dir_permissions`.
    restrict: bool,
}

impl RotatingWriter {
    fn with_policy(dir: PathBuf, prefix: String, restrict: bool) -> io::Result<Self> {
        let active = dir.join(format!("{prefix}.log"));
        let file = open_log_file(&active, restrict)?;
        // If the active file already exists from a prior run, carry its size and
        // attribute its content to the date it was last written; otherwise today.
        let meta = file.metadata()?;
        let size = meta.len();
        let opened_day = file_mtime_date(&meta).unwrap_or_else(|| Local::now().date_naive());
        Ok(Self { dir, prefix, active, file, size, opened_day, restrict })
    }

    /// Rotate the active file out to a dated archive when the day rolled over or
    /// the size cap is hit, then prune archives past the retention window.
    fn maybe_rotate(&mut self) -> io::Result<()> {
        let today = Local::now().date_naive();
        let day_changed = today != self.opened_day;
        let too_big = self.size >= MAX_LOG_SIZE;
        if !day_changed && !too_big {
            return Ok(());
        }

        // Stamp the archive with the day the rotated content belongs to.
        let target = self.archive_path(self.opened_day);
        fs::rename(&self.active, &target)?;

        self.file = open_log_file(&self.active, self.restrict)?;
        self.size = 0;
        self.opened_day = today;

        prune_old(&self.dir, &self.prefix, today);
        Ok(())
    }

    /// `<prefix>.YYYYMMDD.log`, or `<prefix>.YYYYMMDD.N.log` if that day already
    /// has archives (e.g. a second size-triggered rotation on the same day).
    fn archive_path(&self, day: NaiveDate) -> PathBuf {
        let stem = format!("{}.{}", self.prefix, day.format("%Y%m%d"));
        let first = self.dir.join(format!("{stem}.log"));
        if !first.exists() {
            return first;
        }
        let mut n = 1;
        loop {
            let candidate = self.dir.join(format!("{stem}.{n}.log"));
            if !candidate.exists() {
                return candidate;
            }
            n += 1;
        }
    }
}

impl Write for RotatingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Rotate before the write so a rotation never splits a formatted line
        // (the fmt layer writes one event per call).
        self.maybe_rotate()?;
        let n = self.file.write(buf)?;
        self.size += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Open (or create) a log file in append mode at the policy mode.
/// The mode is force-set on every open so a pre-existing file converges to
/// the current policy in both directions.
fn open_log_file(path: &Path, restrict: bool) -> io::Result<File> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mode = log_file_mode(restrict);
        let f = OpenOptions::new().create(true).append(true).mode(mode).open(path)?;
        // Re-apply in case the file pre-existed with a different mode.
        f.set_permissions(std::fs::Permissions::from_mode(mode))?;
        Ok(f)
    }
    #[cfg(not(unix))]
    {
        let _ = restrict;
        OpenOptions::new().create(true).append(true).open(path)
    }
}

/// Local date of a file's last modification, if representable.
fn file_mtime_date(meta: &fs::Metadata) -> Option<NaiveDate> {
    let modified = meta.modified().ok()?;
    let datetime: chrono::DateTime<Local> = modified.into();
    Some(datetime.date_naive())
}

/// Delete archives named `<prefix>.YYYYMMDD[.N].log` whose embedded date is more
/// than [`RETENTION_DAYS`] before `today`. The active `<prefix>.log` has no date
/// component and is never matched. Best-effort: individual failures are ignored.
fn prune_old(dir: &Path, prefix: &str, today: NaiveDate) {
    let cutoff = today - Duration::days(RETENTION_DAYS);
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if let Some(date) = archive_date(&name, prefix)
            && date < cutoff
        {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// Parse the `YYYYMMDD` date from an archive filename `<prefix>.YYYYMMDD[.N].log`.
/// Returns `None` for the active file (`<prefix>.log`) or any non-archive name.
fn archive_date(name: &str, prefix: &str) -> Option<NaiveDate> {
    let rest = name.strip_prefix(prefix)?.strip_prefix('.')?;
    // The date is the first dot-delimited segment after the prefix.
    let date_str = rest.split('.').next()?;
    NaiveDate::parse_from_str(date_str, "%Y%m%d").ok()
}

/// Create the agent log directory at the `restrict_log_dir_permissions`
/// policy mode: `0755` by default (world-readable + traversable so non-root
/// log collectors can read the agent and updater logs), `0750` under opt-in
/// hardening. `create_deployment_dir` force-sets the mode in both directions,
/// covering the upgrade path from either posture.
fn create_agent_log_dir(log_dir: &Path, restrict: bool) -> std::io::Result<()> {
    crate::system::create_deployment_dir(log_dir, 0o750, restrict)
}

/// Initializes the agent log with size+daily file rotation and bounded
/// retention.
///
/// Log format: `{ISO8601} {LEVEL} [{program_name}({pid})]: {message}`
/// Matches the standard agent log format.
///
/// Active file: `<program_name>.log` (stable path, so `tail -f` keeps working).
/// Rotation: at the daily (instance-local) boundary, or early when the active
/// file exceeds 64 MB; the rotated file is `<program_name>.YYYYMMDD.log`
/// (`.N`-suffixed for a second same-day rotation). Archives are pruned after 7
/// days (by the date in their name).
///
/// Retention is age-based rather than count-based: the active file keeps a
/// stable undated name, rotation has both a daily and a size trigger, and
/// archives are named `YYYYMMDD.log` (matching the documented `CodeDeploy`
/// behavior) so they can be pruned by the date they carry.
///
/// Agent log dir `0755`, files `0644` — world-readable so non-root log
/// collectors can tail the agent and updater logs (which hold no sensitive
/// data). Sensitive per-deployment logs live elsewhere and stay restricted.
pub(super) fn init(config: &LogConfig) -> std::io::Result<LogGuard> {
    create_agent_log_dir(&config.log_dir, config.restrict_log_permissions)?;

    let writer = RotatingWriter::with_policy(
        config.log_dir.clone(),
        config.program_name.clone(),
        config.restrict_log_permissions,
    )?;
    let (non_blocking, guard) = tracing_appender::non_blocking(writer);

    let subscriber = tracing_subscriber::registry().with(
        fmt::layer()
            .with_writer(non_blocking)
            .with_timer(build_timer())
            .with_ansi(false)
            .with_target(false)
            .with_filter(build_level_filter(config.verbose)),
    );

    if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
        eprintln!("Failed to set global tracing subscriber: {e}");
        return Err(std::io::Error::other(e.to_string()));
    }

    Ok(LogGuard { _worker_guard: guard })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn verbose_filter_includes_debug() {
        let filter = build_level_filter(true);
        assert!(format!("{filter}").contains("debug"));
    }

    #[test]
    fn non_verbose_filter_is_info() {
        let filter = build_level_filter(false);
        assert!(format!("{filter}").contains("info"));
    }

    #[test]
    fn timer_is_constructable() {
        let _timer = build_timer();
    }

    #[test]
    fn log_guard_is_debuggable() {
        let (_, guard) = tracing_appender::non_blocking(std::io::sink());
        let log_guard = LogGuard { _worker_guard: guard };
        assert!(format!("{log_guard:?}").contains("LogGuard"));
    }

    #[test]
    fn active_file_is_undated() {
        let dir = tempfile::tempdir().unwrap();
        let w =
            RotatingWriter::with_policy(dir.path().to_path_buf(), "codedeploy-agent".into(), false)
                .unwrap();
        assert_eq!(w.active, dir.path().join("codedeploy-agent.log"));
        assert!(w.active.exists());
    }

    #[cfg(unix)]
    #[test]
    fn active_file_created_0644() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let w =
            RotatingWriter::with_policy(dir.path().to_path_buf(), "codedeploy-agent".into(), false)
                .unwrap();
        let mode = fs::metadata(&w.active).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "expected 0644 (world-readable agent log), got {mode:#o}");
    }

    #[cfg(unix)]
    #[test]
    fn agent_log_dir_created_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        create_agent_log_dir(&log_dir, false).unwrap();
        let mode = fs::metadata(&log_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "expected 0755 agent log dir, got {mode:#o}");
    }

    #[cfg(unix)]
    #[test]
    fn agent_log_dir_loosened_on_upgrade_from_0750() {
        use std::os::unix::fs::PermissionsExt;
        // Simulate an older agent that created the dir 0750; the new code must
        // loosen it to 0755 (create_dir_secure alone would leave it 0750).
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).unwrap();
        std::fs::set_permissions(&log_dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        create_agent_log_dir(&log_dir, false).unwrap();
        let mode = fs::metadata(&log_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "expected upgrade to loosen 0750 -> 0755, got {mode:#o}");
    }

    #[cfg(unix)]
    #[test]
    fn agent_log_dir_and_file_hardened_under_restrict_log_flag() {
        // Opt-in `restrict_log_dir_permissions`: dir 0750, active file 0640.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("logs");
        create_agent_log_dir(&log_dir, true).unwrap();
        let dir_mode = fs::metadata(&log_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o750, "expected hardened 0750 agent log dir, got {dir_mode:#o}");

        let w = RotatingWriter::with_policy(log_dir, "codedeploy-agent".into(), true).unwrap();
        let file_mode = fs::metadata(&w.active).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o640, "expected hardened 0640 agent log, got {file_mode:#o}");
    }

    #[cfg(unix)]
    #[test]
    fn agent_log_file_heals_between_policies_on_reopen() {
        // Modes converge to the current policy in both directions on reopen.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("codedeploy-agent.log");

        let _ = open_log_file(&path, true).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640);

        let _ = open_log_file(&path, false).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "expected 0640 -> 0644 heal on default reopen, got {mode:#o}");
    }

    #[test]
    fn size_rotation_archives_with_date_and_reopens_active() {
        let dir = tempfile::tempdir().unwrap();
        let mut w =
            RotatingWriter::with_policy(dir.path().to_path_buf(), "codedeploy-agent".into(), false)
                .unwrap();

        // Force the size trigger and rotate.
        w.size = MAX_LOG_SIZE;
        w.write_all(b"after rotation\n").unwrap();

        // Active file still present and undated, now holding the new line.
        assert!(w.active.exists());
        assert!(fs::read_to_string(&w.active).unwrap().contains("after rotation"));

        // Exactly one dated archive exists for today.
        let today = Local::now().date_naive().format("%Y%m%d").to_string();
        let archive = dir.path().join(format!("codedeploy-agent.{today}.log"));
        assert!(archive.exists(), "expected dated archive {}", archive.display());
    }

    #[test]
    fn same_day_second_rotation_uses_n_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let mut w =
            RotatingWriter::with_policy(dir.path().to_path_buf(), "codedeploy-agent".into(), false)
                .unwrap();
        let today = Local::now().date_naive().format("%Y%m%d").to_string();

        w.size = MAX_LOG_SIZE;
        w.write_all(b"first\n").unwrap();
        w.size = MAX_LOG_SIZE;
        w.write_all(b"second\n").unwrap();

        assert!(dir.path().join(format!("codedeploy-agent.{today}.log")).exists());
        assert!(
            dir.path().join(format!("codedeploy-agent.{today}.1.log")).exists(),
            "second same-day rotation must use a .N suffix"
        );
    }

    #[test]
    fn prune_deletes_archives_older_than_retention() {
        let dir = tempfile::tempdir().unwrap();
        let prefix = "codedeploy-agent";
        let today = NaiveDate::from_ymd_opt(2026, 6, 29).unwrap();

        let old = today - Duration::days(RETENTION_DAYS + 1);
        let recent = today - Duration::days(1);
        let old_file = dir.path().join(format!("{prefix}.{}.log", old.format("%Y%m%d")));
        let recent_file = dir.path().join(format!("{prefix}.{}.log", recent.format("%Y%m%d")));
        let active = dir.path().join(format!("{prefix}.log"));
        fs::write(&old_file, "old").unwrap();
        fs::write(&recent_file, "recent").unwrap();
        fs::write(&active, "active").unwrap();

        prune_old(dir.path(), prefix, today);

        assert!(!old_file.exists(), "archive past retention must be deleted");
        assert!(recent_file.exists(), "archive within retention must be kept");
        assert!(active.exists(), "active file must never be pruned");
    }

    #[test]
    fn archive_date_parses_dated_names_only() {
        let p = "codedeploy-agent";
        assert_eq!(
            archive_date("codedeploy-agent.20260629.log", p),
            Some(NaiveDate::from_ymd_opt(2026, 6, 29).unwrap())
        );
        // .N-suffixed same-day archive still parses the date.
        assert_eq!(
            archive_date("codedeploy-agent.20260629.1.log", p),
            Some(NaiveDate::from_ymd_opt(2026, 6, 29).unwrap())
        );
        // The active file has no date — must not parse.
        assert_eq!(archive_date("codedeploy-agent.log", p), None);
        assert_eq!(archive_date("unrelated.txt", p), None);
    }
}
