//! Host command handling.
//!
//! Routes host command names (`DownloadBundle`, `Install`, lifecycle hooks) to their
//! implementations. Each command is a separate struct with single responsibility.

pub mod appspec_validator;
pub mod archive_reuse;
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

/// Key of the note appended to a successful `DownloadBundle` completion, naming where the bundle
/// came from.
///
/// **Agent → service message format.** Carried inside the existing free-form `message` field of the
/// completion diagnostics, which the service already surfaces verbatim as
/// `"message": "Succeeded: "`:
///
/// ```text
/// {"error_code":0,"script_name":"","message":"Succeeded: BundleSource=archive-reuse","log":""}
/// ```
///
/// The service reads it to count archive-reuse hits, so the key and both values are a contract:
/// renaming either silently zeroes the metric rather than failing a build. Kept in `message` rather
/// than added as a fifth JSON key, because that payload's shape is long-standing and a new key would
/// require the service to tolerate an unknown field.
pub const BUNDLE_SOURCE_NOTE: &str = "BundleSource";

/// Format the completion note for a bundle obtained from `source`.
#[must_use]
pub fn bundle_source_note(source: &str) -> String {
    format!("{BUNDLE_SOURCE_NOTE}={source}")
}

/// Filename, under a deployment's root dir, where `DownloadBundle` records how
/// the bundle was obtained — [`archive_reuse::SOURCE_ARCHIVE_REUSE`] or
/// [`archive_reuse::SOURCE_DOWNLOADED`]. Makes reuse-versus-fallback behaviour
/// observable per host after the fact, not just in logs.
pub const BUNDLE_SOURCE_FILE: &str = ".bundle-source";

#[cfg(test)]
mod bundle_source_note_tests {
    use super::{BUNDLE_SOURCE_NOTE, bundle_source_note};
    use crate::host_command::archive_reuse::{SOURCE_ARCHIVE_REUSE, SOURCE_DOWNLOADED};

    /// The agent -> service message format. Asserted on the literal strings because the service
    /// parses them: renaming the key or either value silently zeroes the reuse metric instead of
    /// failing a build.
    #[test]
    fn the_note_format_is_a_contract() {
        assert_eq!(BUNDLE_SOURCE_NOTE, "BundleSource");
        assert_eq!(bundle_source_note(SOURCE_ARCHIVE_REUSE), "BundleSource=archive-reuse");
        assert_eq!(bundle_source_note(SOURCE_DOWNLOADED), "BundleSource=downloaded");
    }

    /// What the service ends up seeing -- including the empty-note case that every other command
    /// produces, which must stay exactly as it was.
    #[test]
    fn the_note_lands_in_the_diagnostics_message() {
        assert_eq!(
            crate::command_poller::diagnostics::success(&bundle_source_note(SOURCE_ARCHIVE_REUSE)),
            r#"{"error_code":0,"log":"","message":"Succeeded: BundleSource=archive-reuse","script_name":""}"#
        );
        assert_eq!(
            crate::command_poller::diagnostics::success(""),
            r#"{"error_code":0,"log":"","message":"Succeeded","script_name":""}"#
        );
    }
}
