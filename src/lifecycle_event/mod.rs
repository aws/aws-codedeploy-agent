//! @risk medium
//!
//! Lifecycle event execution.
//!
//! Orchestrates running deployment hook scripts. The executor selects the correct
//! deployment directory, parses the appspec, and runs each script for a lifecycle
//! event in order. Script output is streamed to a log file in real-time.

mod bounded_fifo_vec;
pub mod deadline_joiner;
pub mod deployment_selector;
mod deployment_type;
mod error;
mod executor;
mod lifecycle_event_type;
pub mod script;
pub mod script_run_log;

pub use deployment_type::DeploymentType;
pub use error::{ErrorCode, ScriptError};
pub use executor::LifecycleEventExecutor;
pub use lifecycle_event_type::LifecycleEventType;
pub use script::Script;
pub use script_run_log::ScriptRunLog;
