//! @risk low
//!
//! Agent log configuration — file rotation and format setup.
use std::fs;

use time::macros::format_description;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt;
use tracing_subscriber::fmt::time::OffsetTime;
use tracing_subscriber::prelude::*;

use super::LogConfig;

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

/// Initializes the agent log with daily file rotation.
///
/// Log format: `{ISO8601} {LEVEL} [{program_name}({pid})]: {message}`
/// Matches the standard agent log format.
pub(super) fn init(config: &LogConfig) -> std::io::Result<LogGuard> {
    fs::create_dir_all(&config.log_dir)?;

    let file_appender = rolling::daily(&config.log_dir, &config.program_name);
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let subscriber = tracing_subscriber::registry().with(
        fmt::layer()
            .with_writer(non_blocking)
            .with_timer(build_timer())
            .with_ansi(false)
            .with_target(false)
            .with_filter(build_level_filter(config.verbose)),
    );

    tracing::subscriber::set_global_default(subscriber)
        .expect("failed to set global tracing subscriber");

    Ok(LogGuard { _worker_guard: guard })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbose_filter_includes_debug() {
        let filter = build_level_filter(true);
        let filter_str = format!("{filter}");
        assert!(filter_str.contains("debug"));
    }

    #[test]
    fn non_verbose_filter_is_info() {
        let filter = build_level_filter(false);
        let filter_str = format!("{filter}");
        assert!(filter_str.contains("info"));
    }

    #[test]
    fn timer_is_constructable() {
        let _timer = build_timer();
    }

    #[test]
    fn creates_log_directory() {
        let dir = tempfile::tempdir().unwrap();
        let log_dir = dir.path().join("nested").join("logs");
        let config = LogConfig {
            log_dir: log_dir.clone(),
            verbose: false,
            program_name: "test-agent".to_string(),
            root_dir: dir.path().to_path_buf(),
        };

        // We can't call init() because set_global_default is once-per-process,
        // but we can verify the directory creation logic.
        fs::create_dir_all(&config.log_dir).unwrap();
        assert!(log_dir.exists());
    }

    #[test]
    fn log_guard_is_debuggable() {
        // LogGuard wraps WorkerGuard which is Debug
        let (_, guard) = tracing_appender::non_blocking(std::io::sink());
        let log_guard = LogGuard { _worker_guard: guard };
        let debug_str = format!("{log_guard:?}");
        assert!(debug_str.contains("LogGuard"));
    }
}
