//! File copy command — copies files from archive to destination.
use crate::installer::Result;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub struct CopyCommand {
    source: PathBuf,
    destination: PathBuf,
}

impl CopyCommand {
    #[must_use]
    pub fn new(source: PathBuf, destination: PathBuf) -> Self {
        Self { source, destination }
    }

    #[must_use]
    pub fn source(&self) -> &PathBuf {
        &self.source
    }

    #[must_use]
    pub fn destination(&self) -> &PathBuf {
        &self.destination
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, cleanup_file: &mut dyn Write) -> Result<()> {
        writeln!(cleanup_file, "{}", self.destination.display())?;

        let metadata = fs::symlink_metadata(&self.source)?;

        if metadata.is_symlink() {
            let target = fs::read_link(&self.source)?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &self.destination)?;
            #[cfg(windows)]
            {
                // Symlink creation may fail on Windows; fall back to copy
                if std::os::windows::fs::symlink_file(&target, &self.destination).is_err() {
                    fs::copy(&self.source, &self.destination)?;
                }
            }
        } else {
            #[cfg(unix)]
            {
                use nix::sys::stat::{Mode, fchmod};
                use nix::unistd::fchown;
                use std::os::fd::AsRawFd;
                use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};

                // Copy + fchmod/fchown through an O_NOFOLLOW fd (no-follow open).
                // The O_NOFOLLOW open guarantees no-follow; the remove below is a
                // best-effort cleanup of any pre-existing symlink.
                if let Ok(meta) = fs::symlink_metadata(&self.destination)
                    && meta.file_type().is_symlink()
                {
                    fs::remove_file(&self.destination)?;
                }

                let mut src = fs::File::open(&self.source)?;
                let dest = std::fs::OpenOptions::new()
                    .write(true)
                    .create(true)
                    .truncate(true)
                    .mode(0o600)
                    .custom_flags(nix::libc::O_NOFOLLOW)
                    .open(&self.destination)?;

                // Copy source bytes through the fd.
                let mut reader = std::io::BufReader::new(&mut src);
                let mut writer = std::io::BufWriter::new(&dest);
                std::io::copy(&mut reader, &mut writer)?;
                writer.into_inner().map_err(std::io::IntoInnerError::into_error)?;

                // Preserve owner/group on the fd (no-ops if not root). MUST
                // precede fchmod: fchown clears SUID/SGID bits (chown(2)).
                let src_meta = src.metadata()?;
                let _ = fchown(
                    dest.as_raw_fd(),
                    Some(src_meta.uid().into()),
                    Some(src_meta.gid().into()),
                );

                // Preserve permissions on the fd, after fchown so SUID/SGID bits
                // survive. Uses fd-based src_meta so owner, mode, and timestamps
                // share one race-free source.
                let mode = Mode::from_bits_truncate(src_meta.permissions().mode());
                fchmod(dest.as_raw_fd(), mode)
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;

                // Preserve timestamps (atime, mtime) on the fd.
                let atime = nix::sys::time::TimeSpec::new(src_meta.atime(), src_meta.atime_nsec());
                let mtime = nix::sys::time::TimeSpec::new(src_meta.mtime(), src_meta.mtime_nsec());
                let _ = nix::sys::stat::futimens(dest.as_raw_fd(), &atime, &mtime);
            }

            #[cfg(windows)]
            {
                fs::copy(&self.source, &self.destination)?;

                // Windows: preserve timestamps only (no ownership/ACL preservation).
                use filetime::FileTime;
                if let Ok(meta) = fs::metadata(&self.source) {
                    let atime = FileTime::from_last_access_time(&meta);
                    let mtime = FileTime::from_last_modification_time(&meta);
                    let _ = filetime::set_symlink_file_times(&self.destination, atime, mtime);
                }
            }
        }

        Ok(())
    }

    /// Serialize command to JSON hash for debugging/audit
    #[must_use]
    pub fn to_h(&self) -> Value {
        json!({
            "type": "copy",
            "source": self.source,
            "destination": self.destination
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs as unix_fs;

    #[test]
    fn getters() {
        let cmd = CopyCommand::new("/src".into(), "/dst".into());
        assert_eq!(cmd.source(), &std::path::PathBuf::from("/src"));
        assert_eq!(cmd.destination(), &std::path::PathBuf::from("/dst"));
    }

    #[test]
    fn execute_file() {
        let src = std::env::temp_dir().join("test_copy_src.txt");
        let dst = std::env::temp_dir().join("test_copy_dst.txt");
        fs::write(&src, "test").unwrap();

        let cmd = CopyCommand::new(src.clone(), dst.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        assert!(dst.exists());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "test");

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    /// Copying over a symlinked destination lands a real file there (no-follow)
    /// and leaves the symlink's external target intact.
    #[test]
    #[cfg(unix)]
    fn execute_does_not_follow_symlinked_destination() {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::TempDir::new().unwrap();
        let src = dir.path().join("src.txt");
        fs::write(&src, "payload").unwrap();

        // An external file the destination symlink points at; must stay intact.
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, "original").unwrap();

        // Destination is pre-occupied by a symlink to the external file.
        let dst = dir.path().join("dst.txt");
        unix_fs::symlink(&outside, &dst).unwrap();

        let cmd = CopyCommand::new(src.clone(), dst.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        // The external target retains its original bytes.
        assert_eq!(fs::read_to_string(&outside).unwrap(), "original");
        // The destination is now a real regular file holding the source bytes.
        let meta = fs::symlink_metadata(&dst).unwrap();
        assert!(!meta.file_type().is_symlink(), "destination must not be a symlink");
        assert_eq!(fs::read_to_string(&dst).unwrap(), "payload");
        // Source ownership preserved onto the fd (uid matches current process).
        assert_eq!(meta.uid(), nix::unistd::Uid::current().as_raw());
    }

    #[test]
    #[cfg(unix)]
    fn execute_symlink() {
        let target = std::env::temp_dir().join("test_copy_target.txt");
        let src = std::env::temp_dir().join("test_copy_link.txt");
        let dst = std::env::temp_dir().join("test_copy_link_dst.txt");

        fs::write(&target, "test").unwrap();
        unix_fs::symlink(&target, &src).unwrap();

        let cmd = CopyCommand::new(src.clone(), dst.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        assert!(dst.exists());
        assert!(fs::symlink_metadata(&dst).unwrap().is_symlink());

        fs::remove_file(&target).ok();
        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }

    #[test]
    fn execute_nonexistent_source() {
        let cmd = CopyCommand::new("/nonexistent".into(), "/dst".into());
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());
    }

    #[test]
    fn to_h() {
        let cmd = CopyCommand::new("/source/file.txt".into(), "/dest/file.txt".into());
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "copy");
        assert_eq!(hash["source"], "/source/file.txt");
        assert_eq!(hash["destination"], "/dest/file.txt");
    }

    #[test]
    #[cfg(unix)]
    fn execute_preserves_timestamps() {
        use std::os::unix::fs::MetadataExt;
        use std::thread;
        use std::time::Duration;

        let src = std::env::temp_dir().join("test_preserve_src.txt");
        fs::write(&src, "test").unwrap();

        // Wait a bit to ensure different timestamp
        thread::sleep(Duration::from_millis(100));

        let dst = std::env::temp_dir().join("test_preserve_dst.txt");
        let cmd = CopyCommand::new(src.clone(), dst.clone());
        let mut cleanup = Vec::new();
        cmd.execute(&mut cleanup).unwrap();

        let src_meta = fs::metadata(&src).unwrap();
        let dst_meta = fs::metadata(&dst).unwrap();

        // Timestamps should match (within 1 second tolerance for filesystem precision)
        assert!((src_meta.mtime() - dst_meta.mtime()).abs() <= 1);

        fs::remove_file(&src).ok();
        fs::remove_file(&dst).ok();
    }
}
