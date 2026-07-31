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

/// Filename, under a deployment's root dir, where `DownloadBundle` records the
/// S3 object's `ETag` so the executor can expose it to hooks as `BUNDLE_ETAG`
/// even when the spec carried a null `ETag`.
pub const BUNDLE_ETAG_FILE: &str = ".bundle-etag";
