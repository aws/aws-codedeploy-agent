//! @risk high
//!
//! Host command handling.
//!
//! Routes host command names (`DownloadBundle`, `Install`, lifecycle hooks) to their
//! implementations. Each command is a separate struct with single responsibility.

pub mod appspec_validator;
pub mod bundle_downloader;
pub mod bundle_unpacker;
mod command_dispatcher;
pub mod commands;
mod deployment_archives;

pub use command_dispatcher::CommandDispatcher;
pub use deployment_archives::DeploymentArchives;
