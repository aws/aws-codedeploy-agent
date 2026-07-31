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
    restrict_permissions: bool,
}

impl LocalDirectoryDownloader {
    #[must_use]
    pub fn new(source: PathBuf, dest: PathBuf, restrict_permissions: bool) -> Self {
        Self { source, dest, restrict_permissions }
    }
}

impl BundleDownloader for LocalDirectoryDownloader {
    fn download(&self) -> io::Result<()> {
        copy_dir_recursive(&self.source, &self.dest)?;

        // Force the archive root to the policy mode (0755 by default, or
        // 0711 traversable-not-listable under opt-in hardening), matching
        // bundle_unpacker::unpack. The copy preserves the source top-dir mode,
        // often 0700 from `mktemp -d`, which would block runas: traversal.
        #[cfg(unix)]
        {
            use std::fs;
            use std::os::unix::fs::PermissionsExt;
            let mode =
                crate::host_command::bundle_unpacker::archive_dir_mode(self.restrict_permissions);
            fs::set_permissions(&self.dest, fs::Permissions::from_mode(mode))?;
        }

        Ok(())
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
        LocalDirectoryDownloader::new(source, dest.clone(), false).download().unwrap();

        assert_eq!(std::fs::read_to_string(dest.join("a.txt")).unwrap(), "hello");
        assert_eq!(std::fs::read_to_string(dest.join("sub/b.txt")).unwrap(), "world");
    }

    #[test]
    fn missing_source_fails() {
        let dir = TempDir::new().unwrap();
        let result = LocalDirectoryDownloader::new(
            dir.path().join("missing"),
            dir.path().join("dest"),
            false,
        )
        .download();
        assert!(result.is_err());
    }
}
