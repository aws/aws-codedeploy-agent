//! @risk low
//!
//! Bundle downloader factory.
use std::path::{Path, PathBuf};

use super::error::RuntimeError;

/// Bundle format types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleFormat {
    Tar,
    Tgz,
    Zip,
    Directory,
}

/// Downloaded bundle information
#[derive(Debug, Clone)]
pub struct Bundle {
    pub path: PathBuf,
    pub format: BundleFormat,
}

/// Trait for downloading deployment bundles from various sources
pub trait BundleDownloader: Send + Sync {
    /// Download a bundle from the specified location to the destination directory
    /// Returns the path to the downloaded bundle and its format
    /// # Errors
    /// Returns an error if download fails.
    fn download(&self, location: &str, dest_dir: &Path) -> Result<Bundle, RuntimeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundle_format_equality() {
        assert_eq!(BundleFormat::Tar, BundleFormat::Tar);
        assert_ne!(BundleFormat::Tar, BundleFormat::Zip);
        assert_ne!(BundleFormat::Tgz, BundleFormat::Directory);
    }

    #[test]
    fn bundle_format_is_debuggable() {
        assert_eq!(format!("{:?}", BundleFormat::Tar), "Tar");
        assert_eq!(format!("{:?}", BundleFormat::Tgz), "Tgz");
        assert_eq!(format!("{:?}", BundleFormat::Zip), "Zip");
        assert_eq!(format!("{:?}", BundleFormat::Directory), "Directory");
    }

    #[test]
    fn bundle_format_is_copyable() {
        let fmt = BundleFormat::Tgz;
        let copied = fmt;
        assert_eq!(fmt, copied);
    }

    #[test]
    fn bundle_is_cloneable() {
        let b = Bundle { path: PathBuf::from("/tmp/bundle.tar"), format: BundleFormat::Tar };
        let cloned = b.clone();
        assert_eq!(cloned.path, PathBuf::from("/tmp/bundle.tar"));
        assert_eq!(cloned.format, BundleFormat::Tar);
    }

    #[test]
    fn bundle_is_debuggable() {
        let b = Bundle { path: PathBuf::from("/tmp/b.zip"), format: BundleFormat::Zip };
        let debug = format!("{b:?}");
        assert!(debug.contains("b.zip"));
        assert!(debug.contains("Zip"));
    }
}
