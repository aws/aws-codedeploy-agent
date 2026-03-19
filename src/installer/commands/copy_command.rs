//! @risk medium
//!
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
            fs::copy(&self.source, &self.destination)?;

            #[cfg(unix)]
            {
                use nix::fcntl::AtFlags;
                use nix::sys::stat::{UtimensatFlags, fstatat, utimensat};
                use nix::sys::time::TimeSpec;
                use nix::unistd::chown;

                // Preserve permissions
                let perms = metadata.permissions();
                fs::set_permissions(&self.destination, perms)?;

                // Get source file stat for timestamps and ownership
                let stat = fstatat(None, &self.source, AtFlags::AT_SYMLINK_NOFOLLOW)
                    .map_err(|e| std::io::Error::from_raw_os_error(e as i32))?;

                // Preserve timestamps (atime, mtime)
                let atime = TimeSpec::new(stat.st_atime, stat.st_atime_nsec);
                let mtime = TimeSpec::new(stat.st_mtime, stat.st_mtime_nsec);
                let _ = utimensat(
                    None,
                    &self.destination,
                    &atime,
                    &mtime,
                    UtimensatFlags::NoFollowSymlink,
                );

                // Preserve owner/group (will silently fail if not root)
                let _ =
                    chown(&self.destination, Some(stat.st_uid.into()), Some(stat.st_gid.into()));
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

    #[test]
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
