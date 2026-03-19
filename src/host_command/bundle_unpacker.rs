//! @risk medium
//!
//! Bundle archive unpacking.
//!
//! Extracts tar/tgz/zip archives and strips a leading directory if the archive
//! contains a single top-level folder with an appspec file.
//!
//! Uses system commands (tar, unzip) rather than Rust crates to match existing
//! behavior. These tools are always available on EC2 instances.

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;
use tracing::debug;

/// Unpack a bundle archive into `dest`, then strip any leading directory.
///
/// # Errors
/// Returns an error if extraction or directory stripping fails.
pub fn unpack(bundle_path: &Path, dest: &Path, bundle_type: &str) -> io::Result<()> {
    debug!(
        bundle_type,
        bundle = %bundle_path.display(),
        dest = %dest.display(),
        "Unpacking bundle archive"
    );
    fs::create_dir_all(dest)?;

    match bundle_type {
        "tgz" => unpack_tgz(bundle_path, dest)?,
        "zip" => unpack_zip(bundle_path, dest)?,
        // "tar" and anything else default to tar
        _ => unpack_tar(bundle_path, dest)?,
    }

    strip_leading_directory(dest)
}

fn unpack_tar(bundle: &Path, dest: &Path) -> io::Result<()> {
    run_command(
        "tar",
        &[
            "-xf",
            &bundle.display().to_string(),
            "-C",
            &dest.display().to_string(),
        ],
    )
}

fn unpack_tgz(bundle: &Path, dest: &Path) -> io::Result<()> {
    run_command(
        "tar",
        &[
            "-xzf",
            &bundle.display().to_string(),
            "-C",
            &dest.display().to_string(),
        ],
    )
}

fn unpack_zip(bundle: &Path, dest: &Path) -> io::Result<()> {
    let output = Command::new("unzip")
        .args([
            "-o",
            &bundle.display().to_string(),
            "-d",
            &dest.display().to_string(),
        ])
        .output()?;

    if !output.status.success() {
        // Exit code 50 = disk full. See http://infozip.sourceforge.net/FAQ.html#error-codes
        if output.status.code() == Some(50) {
            let _ = fs::remove_dir_all(dest);
            return Err(io::Error::other("The disk is (or was) full during extraction."));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        // TODO: Fall back to native zip extraction (zip crate) for partial/skipped files.
        // TODO: Fall back to native zip extraction for partial/skipped files.
        return Err(io::Error::other(format!(
            "unzip failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    Ok(())
}

fn run_command(program: &str, args: &[&str]) -> io::Result<()> {
    let output = Command::new(program).args(args).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::other(format!(
            "{program} failed (exit {}): {stderr}",
            output.status.code().unwrap_or(-1)
        )));
    }

    Ok(())
}

/// If the archive has a single top-level directory containing an appspec,
/// move its contents up one level (strip the wrapper directory).
fn strip_leading_directory(dest: &Path) -> io::Result<()> {
    let entries: Vec<_> = fs::read_dir(dest)?.filter_map(Result::ok).map(|e| e.path()).collect();

    if entries.len() != 1 || !entries[0].is_dir() {
        return Ok(());
    }

    let inner = &entries[0];

    // Check if inner dir contains an appspec file
    let has_appspec = fs::read_dir(inner)?
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().to_lowercase().contains("appspec"));

    if !has_appspec {
        return Ok(());
    }

    debug!("Stripping leading directory from archive bundle contents.");

    let temp = dest.with_file_name("deployment-archive-temp");
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    fs::rename(dest, &temp)?;
    fs::rename(temp.join(inner.file_name().unwrap()), dest)?;
    fs::remove_dir(&temp)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn strip_single_dir_with_appspec() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("deployment-archive");
        let inner = dest.join("my-app");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("appspec.yml"), "version: 0.0").unwrap();
        fs::write(inner.join("script.sh"), "#!/bin/sh").unwrap();

        strip_leading_directory(&dest).unwrap();

        assert!(dest.join("appspec.yml").exists());
        assert!(dest.join("script.sh").exists());
        assert!(!dest.join("my-app").exists());
    }

    #[test]
    fn strip_no_op_multiple_entries() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("deployment-archive");
        fs::create_dir_all(dest.join("dir1")).unwrap();
        fs::create_dir_all(dest.join("dir2")).unwrap();

        strip_leading_directory(&dest).unwrap();

        assert!(dest.join("dir1").exists());
        assert!(dest.join("dir2").exists());
    }

    #[test]
    fn strip_no_op_single_dir_without_appspec() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("deployment-archive");
        let inner = dest.join("my-app");
        fs::create_dir_all(&inner).unwrap();
        fs::write(inner.join("readme.txt"), "hello").unwrap();

        strip_leading_directory(&dest).unwrap();

        // Not stripped — no appspec
        assert!(dest.join("my-app").exists());
    }

    #[test]
    fn strip_no_op_single_file() {
        let dir = TempDir::new().unwrap();
        let dest = dir.path().join("deployment-archive");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("appspec.yml"), "version: 0.0").unwrap();

        strip_leading_directory(&dest).unwrap();

        // Single file, not a dir — no stripping
        assert!(dest.join("appspec.yml").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unpack_tar_roundtrip() {
        let dir = TempDir::new().unwrap();

        // Create a tar with a file
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        unpack(&tar_path, &dest, "tar").unwrap();

        assert!(dest.join("hello.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unpack_tgz_roundtrip() {
        let dir = TempDir::new().unwrap();

        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let tgz_path = dir.path().join("bundle.tgz");
        Command::new("tar")
            .args([
                "-czf",
                &tgz_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        unpack(&tgz_path, &dest, "tgz").unwrap();

        assert!(dest.join("hello.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn unpack_unknown_type_defaults_to_tar() {
        let dir = TempDir::new().unwrap();

        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("hello.txt"), "world").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");
        unpack(&tar_path, &dest, "unknown").unwrap();

        assert!(dest.join("hello.txt").exists());
    }

    #[cfg(unix)]
    #[test]
    fn strip_leading_directory_cleans_up_existing_temp() {
        let dir = TempDir::new().unwrap();

        let src = dir.path().join("wrapper");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("appspec.yml"), "version: 0.0").unwrap();
        fs::write(src.join("file.txt"), "content").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &dir.path().display().to_string(),
                "wrapper",
            ])
            .output()
            .unwrap();

        let dest = dir.path().join("out");

        // Create temp directory that should be cleaned up
        let temp = dest.with_file_name("deployment-archive-temp");
        fs::create_dir_all(&temp).unwrap();

        unpack(&tar_path, &dest, "tar").unwrap();

        // Verify temp was cleaned up and files were stripped
        assert!(!temp.exists());
        assert!(dest.join("appspec.yml").exists());
        assert!(dest.join("file.txt").exists());
    }
}
