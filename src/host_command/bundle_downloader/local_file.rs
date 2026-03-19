//! @risk medium
//!
//! Local file downloader — symlink (Unix) or copy (Windows).

use super::BundleDownloader;
use std::io;
use std::path::PathBuf;

#[derive(Debug)]
pub struct LocalFileDownloader {
    source: PathBuf,
    dest: PathBuf,
}

impl LocalFileDownloader {
    #[must_use]
    pub fn new(source: PathBuf, dest: PathBuf) -> Self {
        Self { source, dest }
    }
}

impl BundleDownloader for LocalFileDownloader {
    fn download(&self) -> io::Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&self.source, &self.dest)
        }
        #[cfg(not(unix))]
        {
            std::fs::copy(&self.source, &self.dest).map(|_| ())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[cfg(unix)]
    #[test]
    fn creates_symlink() {
        let dir = TempDir::new().unwrap();
        let source = dir.path().join("bundle.tar");
        std::fs::write(&source, "data").unwrap();

        let dest = dir.path().join("link.tar");
        LocalFileDownloader::new(source, dest.clone()).download().unwrap();

        assert!(dest.is_symlink());
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "data");
    }

    #[cfg(unix)]
    #[test]
    fn missing_source_creates_dangling_symlink() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("link.tar");
        LocalFileDownloader::new(dir.path().join("missing"), dest.clone())
            .download()
            .unwrap();
        assert!(dest.is_symlink());
    }
}
