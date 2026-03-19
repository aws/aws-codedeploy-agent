//! @risk medium
//!
//! Local directory downloader — recursive copy.
//!
//! Copies instead of symlinking to preserve revision history.

use super::BundleDownloader;
use crate::system::file_ops::copy_dir_recursive;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub struct LocalDirectoryDownloader {
    source: PathBuf,
    dest: PathBuf,
}

impl LocalDirectoryDownloader {
    #[must_use]
    pub fn new(source: PathBuf, dest: PathBuf) -> Self {
        Self { source, dest }
    }
}

impl BundleDownloader for LocalDirectoryDownloader {
    fn download(&self) -> io::Result<()> {
        copy_dir_recursive(&self.source, &self.dest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn copies_recursively() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("src");
        std::fs::create_dir_all(source.join("sub")).unwrap();
        std::fs::write(source.join("a.txt"), "hello").unwrap();
        std::fs::write(source.join("sub/b.txt"), "world").unwrap();

        let dest = dir.path().join("dest");
        LocalDirectoryDownloader::new(source, dest.clone()).download().unwrap();

        assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "hello");
        assert_eq!(std::fs::read_to_string(dest.join("sub/b.txt")).unwrap(), "world");
    }

    #[test]
    fn missing_source_fails() {
        let dir = TempDir::new().unwrap();
        let result =
            LocalDirectoryDownloader::new(dir.path().join("missing"), dir.path().join("dest"))
                .download();
        assert!(result.is_err());
    }
}
