//! Bundle downloading from various sources.
//!
//! Each downloader is constructed with all config it needs, then `download()` executes.
//! CE dispatches to the right downloader based on `revision_source`.

mod github;
mod local_directory;
mod local_file;
mod s3;

use std::io;

/// Common interface for all bundle downloaders.
pub trait BundleDownloader {
    /// Download the bundle to the configured destination.
    ///
    /// # Errors
    /// Returns an error if the download fails.
    fn download(&self) -> io::Result<()>;
}

pub use github::{BundleFormat, GitHubDownloader};
pub use local_directory::LocalDirectoryDownloader;
pub use local_file::LocalFileDownloader;
pub use s3::S3Downloader;
