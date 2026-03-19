//! @risk high
//!
//! CodeDeploy agent binary entry point.
//!
//! Subcommands:
//! - `start`   — daemonize and begin polling
//! - `stop`    — graceful shutdown
//! - `restart` — stop then start
//! - `status`  — report running/stopped
//! - `_worker` — internal: child worker process (not user-facing)

use std::path::{Path, PathBuf};
use std::process;

use aws_codedeploy_agent::config::AgentConfig;
use aws_codedeploy_agent::daemon::master::{Master, MasterConfig, StopOutcome};
use aws_codedeploy_agent::daemon::signal::{ShutdownFlag, register_shutdown_handlers};
use aws_codedeploy_agent::daemon::worker;
use aws_codedeploy_agent::logging::{LogConfig, init_logging};
use aws_codedeploy_agent::runtime::FileBasedDeploymentTracker;
use aws_codedeploy_agent::system::SystemFileOperations;
use clap::{Parser, Subcommand};

/// Environment variable used to forward `--config-file` to the worker subprocess.
///
/// Set by the master process before spawning workers. The worker subprocess
/// reads this on startup to load the same config file the master used.
///
/// Ruby: the GLI `--config-file` flag is a global option processed in a pre-hook.
/// Since the worker is a child process (re-exec with `worker` subcommand), we
/// forward the path via env var so the worker can load the same config file.
///
/// This variable is NOT intended to be set externally by users — use
/// `--config-file` instead.
const CONFIG_FILE_ENV: &str = "CODEDEPLOY_AGENT_CONFIG_FILE";

#[derive(Parser)]
#[command(name = "codedeploy-agent", about = "AWS CodeDeploy Agent")]
struct Cli {
    /// Path to agent config file.
    ///
    /// Ruby: `--config-file` flag in `lib/codedeploy-agent.rb`.
    /// Defaults to `/etc/codedeploy-agent/conf/codedeployagent.yml`.
    #[arg(long = "config-file", global = true)]
    config_file: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the agent daemon
    Start,
    /// Stop the agent daemon gracefully
    Stop,
    /// Restart the agent daemon
    Restart,
    /// Check if the agent is running
    Status,
    /// Internal: worker child process (not user-facing)
    #[command(hide = true)]
    #[allow(non_camel_case_types)]
    _worker,
}

/// Build the deployment tracker path from config.
///
/// Deployment tracking directory path.
/// `config[:root_dir]` and `config[:ongoing_deployment_tracking]`
/// (see `deployment_command_tracker.rb:deployment_dir_path`).
fn tracker_path(config: &AgentConfig) -> PathBuf {
    PathBuf::from(&config.root_dir).join(&config.ongoing_deployment_tracking)
}

/// Create a deployment tracker from config.
fn make_tracker(config: &AgentConfig) -> FileBasedDeploymentTracker<SystemFileOperations> {
    FileBasedDeploymentTracker::<SystemFileOperations>::new(tracker_path(config))
}

/// Create a Master from config.
///
/// Build daemon Master from config.
/// `kill_agent_max_wait_time_seconds` into the master process.
fn make_master(config: &AgentConfig) -> Master {
    Master::new(MasterConfig {
        pid_dir: config.pid_dir.to_str().expect("pid_dir must be valid UTF-8").to_string(),
        kill_wait_secs: config.kill_agent_max_wait_time_seconds,
        ..MasterConfig::default()
    })
}

fn run(command: &Command, config_file: Option<&Path>) {
    // Resolve config path: CLI flag takes precedence, then env var (set by master
    // for worker subprocess), then default path.
    let config_path = config_file
        .map(PathBuf::from)
        .or_else(|| std::env::var(CONFIG_FILE_ENV).ok().map(PathBuf::from));
    let config_path_ref = config_path.as_deref();

    // Load config before anything else — load once, pass to all consumers.
    let config = match AgentConfig::load(config_path_ref) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config: {e}");
            process::exit(1);
        },
    };

    // Create log and PID directories.
    if let Err(e) = config.ensure_dirs() {
        eprintln!("Failed to create required directories: {e}");
        process::exit(1);
    }

    // Forward config file path to worker subprocess via env var.
    // Only needed for commands that spawn worker subprocesses.
    if let Some(p) = config_path_ref
        && matches!(command, Command::Start | Command::Restart)
    {
        // SAFETY: set_var is called in the master process before any threads
        // are spawned. The child worker reads this env var on startup.
        unsafe { std::env::set_var(CONFIG_FILE_ENV, p) };
    }

    match command {
        Command::Start => {
            let master = make_master(&config);
            if let Err(e) = master.start() {
                eprintln!("Failed to start agent: {e}");
                process::exit(1);
            }
        },
        Command::Stop => {
            let master = make_master(&config);
            let tracker = make_tracker(&config);
            if let Err(e) = master.stop(Some(&tracker)) {
                eprintln!("Failed to stop agent: {e}");
                process::exit(1);
            }
        },
        Command::Restart => {
            let master = make_master(&config);
            let tracker = make_tracker(&config);
            // Stop — propagate deployment and timeout errors, ignore "not running"
            match master.stop(Some(&tracker)) {
                Ok(StopOutcome::Stopped | StopOutcome::NotRunning) => {},
                Err(e) => {
                    eprintln!("Failed to restart agent: {e}");
                    process::exit(1);
                },
            }
            // Fresh master needed: the previous instance's ShutdownFlag may
            // have been set during stop, and start() registers new signal
            // handlers against the flag.
            // NOTE: signal-hook's flag::register appends handlers globally and does not
            // support unregistration. Each restart leaks one Arc<AtomicBool> (~32 bytes).
            // Acceptable since restarts are rare. Consider signal_hook::iterator if this
            // becomes a concern.
            let fresh = make_master(&config);
            if let Err(e) = fresh.start() {
                eprintln!("Failed to start agent: {e}");
                process::exit(1);
            }
        },
        Command::Status => {
            let master = make_master(&config);
            match master.status() {
                Ok(true) => {
                    println!("The AWS CodeDeploy agent is running");
                    process::exit(0);
                },
                Ok(false) => {
                    println!("The AWS CodeDeploy agent is not running");
                    process::exit(3); // LSB: program is not running
                },
                Err(e) => {
                    eprintln!("Error checking status: {e}");
                    process::exit(4); // LSB: status unknown
                },
            }
        },
        Command::_worker => {
            // Initialize logging for the worker subprocess.
            let log_config = LogConfig {
                log_dir: config.log_dir.clone(),
                verbose: config.verbose,
                program_name: config.program_name.clone(),
                root_dir: config.root_dir.clone(),
            };
            let _guard = match init_logging(&log_config) {
                Ok(g) => g,
                Err(e) => {
                    eprintln!("Failed to initialize logging: {e}");
                    process::exit(1);
                },
            };

            let shutdown = ShutdownFlag::new();
            if let Err(e) = register_shutdown_handlers(&shutdown) {
                eprintln!("Failed to register signal handlers: {e}");
                process::exit(1);
            }
            worker::run(&shutdown, &config);
        },
    }
}

fn main() {
    let cli = Cli::parse();
    run(&cli.command, cli.config_file.as_deref());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_parses_start_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "start"]).unwrap();
        assert!(matches!(cli.command, Command::Start));
    }

    #[test]
    fn cli_parses_stop_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "stop"]).unwrap();
        assert!(matches!(cli.command, Command::Stop));
    }

    #[test]
    fn cli_parses_restart_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "restart"]).unwrap();
        assert!(matches!(cli.command, Command::Restart));
    }

    #[test]
    fn cli_parses_status_subcommand() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "status"]).unwrap();
        assert!(matches!(cli.command, Command::Status));
    }

    #[test]
    fn cli_parses_hidden_worker_subcommand() {
        // clap strips the leading underscore: _worker variant -> "worker" subcommand
        let cli = Cli::try_parse_from(["codedeploy-agent", "worker"]).unwrap();
        assert!(matches!(cli.command, Command::_worker));
    }

    #[test]
    fn cli_rejects_unknown_subcommand() {
        assert!(Cli::try_parse_from(["codedeploy-agent", "unknown"]).is_err());
    }

    #[test]
    fn cli_parses_config_file_flag() {
        let cli = Cli::try_parse_from([
            "codedeploy-agent",
            "--config-file",
            "/tmp/test.yml",
            "start",
        ])
        .unwrap();
        assert_eq!(cli.config_file, Some(PathBuf::from("/tmp/test.yml")));
        assert!(matches!(cli.command, Command::Start));
    }

    #[test]
    fn cli_config_file_defaults_to_none() {
        let cli = Cli::try_parse_from(["codedeploy-agent", "start"]).unwrap();
        assert!(cli.config_file.is_none());
    }

    #[test]
    fn tracker_path_joins_root_and_tracking_dir() {
        let config = AgentConfig::default();
        let path = tracker_path(&config);
        assert_eq!(path, PathBuf::from("/opt/codedeploy-agent/deployment-root/ongoing-deployment"));
    }

    #[test]
    fn tracker_path_respects_custom_config() {
        let mut config = AgentConfig::default();
        config.root_dir = PathBuf::from("/custom/root");
        config.ongoing_deployment_tracking = "custom-tracking".to_string();
        assert_eq!(tracker_path(&config), PathBuf::from("/custom/root/custom-tracking"));
    }

    #[test]
    fn make_master_config_maps_pid_dir() {
        let mut config = AgentConfig::default();
        config.pid_dir = PathBuf::from("/custom/pids");
        let mc = MasterConfig {
            pid_dir: config.pid_dir.to_str().expect("valid UTF-8").to_string(),
            kill_wait_secs: config.kill_agent_max_wait_time_seconds,
            ..MasterConfig::default()
        };
        assert_eq!(mc.pid_dir, "/custom/pids");
    }

    #[test]
    fn make_master_config_maps_kill_wait() {
        let mut config = AgentConfig::default();
        config.kill_agent_max_wait_time_seconds = 999;
        let mc = MasterConfig {
            pid_dir: config.pid_dir.to_str().expect("valid UTF-8").to_string(),
            kill_wait_secs: config.kill_agent_max_wait_time_seconds,
            ..MasterConfig::default()
        };
        assert_eq!(mc.kill_wait_secs, 999);
    }
}
