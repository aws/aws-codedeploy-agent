//! @risk high
//!
//! Command polling and processing.
//!
//! Entry point for the agent's main loop. Polls the `CodeDeploy` service for
//! pending host commands, then processes each one through the command pipeline.

pub mod backoff;
pub mod command_processor;
pub mod crash_recovery;
pub mod diagnostics;
pub mod host_command_poller;

pub use command_processor::CommandProcessor;
pub use host_command_poller::{CancelToken, HostCommandPoller};
