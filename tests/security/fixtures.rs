// Adversarial input generators for security tests.
// Generates malicious archives, bad AppSpec files, and other adversarial test inputs
// programmatically — no malicious fixture files are checked into the repository.
//


use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Archive fixtures — tar files with adversarial entries
// ---------------------------------------------------------------------------

/// Build a tar archive containing a file with a literal traversal path.
///
/// Writes a raw POSIX ustar header naming `traversal_path` directly — system
/// `tar -cf` rewrites `../` paths during creation, so we cannot rely on it to
/// embed a malicious entry name. The raw-header approach guarantees the
/// archive's on-wire entry path is exactly `traversal_path`.
///
/// # Examples
///
/// ```rust,no_run
/// let (_dir, tar) = fixtures::tar_with_traversal("../../../etc/passwd", b"pwned").unwrap();
/// ```
pub fn tar_with_traversal(traversal_path: &str, content: &[u8]) -> io::Result<(TempDir, PathBuf)> {
    let dir = TempDir::new()?;
    let tar_path = dir.path().join("malicious.tar");
    let mut f = std::fs::File::create(&tar_path)?;
    f.write_all(&tar_file_header(traversal_path, content.len() as u64))?;
    f.write_all(content)?;
    f.write_all(&tar_padding(content.len() as u64))?;
    f.write_all(&tar_end_marker())?;
    Ok((dir, tar_path))
}

// ---------------------------------------------------------------------------
// Raw tar helpers (no external crate — uses POSIX ustar format)
// ---------------------------------------------------------------------------

/// Compute the POSIX ustar checksum for a 512-byte header block.
/// The checksum field (bytes 148..156) must be filled with spaces before calling.
fn tar_checksum(header: &[u8; 512]) -> String {
    let cksum: u32 = header.iter().map(|&b| b as u32).sum();
    format!("{:06o}\0 ", cksum)
}

/// Write a POSIX ustar header for a regular file entry.
fn tar_file_header(path: &str, size: u64) -> [u8; 512] {
    let mut header = [0u8; 512];
    // name (0..100)
    let name_bytes = path.as_bytes();
    let len = name_bytes.len().min(100);
    header[..len].copy_from_slice(&name_bytes[..len]);
    // mode (100..108) — 0644
    header[100..107].copy_from_slice(b"0000644");
    // uid (108..116)
    header[108..115].copy_from_slice(b"0001000");
    // gid (116..124)
    header[116..123].copy_from_slice(b"0001000");
    // size (124..136) — octal
    let size_str = format!("{:011o}", size);
    header[124..135].copy_from_slice(size_str.as_bytes());
    // mtime (136..148)
    header[136..147].copy_from_slice(b"14717450000");
    // typeflag (156) — '0' = regular file
    header[156] = b'0';
    // magic (257..263) — "ustar\0"
    header[257..263].copy_from_slice(b"ustar\0");
    // version (263..265)
    header[263..265].copy_from_slice(b"00");
    // checksum (148..156) — compute with field as spaces
    header[148..156].copy_from_slice(b"        ");
    let cksum_str = tar_checksum(&header);
    header[148..156].copy_from_slice(cksum_str.as_bytes());
    header
}

/// Write a POSIX ustar header for a symlink entry.
fn tar_symlink_header(path: &str, target: &str) -> [u8; 512] {
    let mut header = [0u8; 512];
    let name_bytes = path.as_bytes();
    let len = name_bytes.len().min(100);
    header[..len].copy_from_slice(&name_bytes[..len]);
    // mode (100..108) — 0777 for symlinks
    header[100..107].copy_from_slice(b"0000777");
    // uid/gid
    header[108..115].copy_from_slice(b"0001000");
    header[116..123].copy_from_slice(b"0001000");
    // size = 0 for symlinks
    header[124..135].copy_from_slice(b"00000000000");
    // mtime
    header[136..147].copy_from_slice(b"14717450000");
    // typeflag (156) — '2' = symlink
    header[156] = b'2';
    // linkname (157..257)
    let target_bytes = target.as_bytes();
    let tlen = target_bytes.len().min(100);
    header[157..157 + tlen].copy_from_slice(&target_bytes[..tlen]);
    // magic + version
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    // checksum
    header[148..156].copy_from_slice(b"        ");
    let cksum_str = tar_checksum(&header);
    header[148..156].copy_from_slice(cksum_str.as_bytes());
    header
}

/// Write a POSIX ustar header for a hardlink entry.
fn tar_hardlink_header(path: &str, target: &str) -> [u8; 512] {
    let mut header = [0u8; 512];
    let name_bytes = path.as_bytes();
    let len = name_bytes.len().min(100);
    header[..len].copy_from_slice(&name_bytes[..len]);
    header[100..107].copy_from_slice(b"0000644");
    header[108..115].copy_from_slice(b"0001000");
    header[116..123].copy_from_slice(b"0001000");
    header[124..135].copy_from_slice(b"00000000000");
    header[136..147].copy_from_slice(b"14717450000");
    // typeflag (156) — '1' = hardlink
    header[156] = b'1';
    // linkname (157..257)
    let target_bytes = target.as_bytes();
    let tlen = target_bytes.len().min(100);
    header[157..157 + tlen].copy_from_slice(&target_bytes[..tlen]);
    header[257..263].copy_from_slice(b"ustar\0");
    header[263..265].copy_from_slice(b"00");
    // checksum
    header[148..156].copy_from_slice(b"        ");
    let cksum_str = tar_checksum(&header);
    header[148..156].copy_from_slice(cksum_str.as_bytes());
    header
}

/// Pad data to a 512-byte boundary (tar record alignment).
fn tar_padding(data_len: u64) -> Vec<u8> {
    let remainder = data_len % 512;
    if remainder == 0 {
        Vec::new()
    } else {
        vec![0u8; (512 - remainder) as usize]
    }
}

/// Write the two-block end-of-archive marker.
fn tar_end_marker() -> [u8; 1024] {
    [0u8; 1024]
}

// ---------------------------------------------------------------------------
// Archive generators
// ---------------------------------------------------------------------------

/// Create a tar archive containing entries with path traversal sequences.
///
/// Includes: `../../../etc/passwd`, an absolute path `/etc/shadow`,
/// URL-encoded traversal `%2e%2e%2f`, and intermediate traversal
/// `subdir/../../../etc/passwd`.
pub fn tar_with_path_traversal(dest: &Path) -> PathBuf {
    let archive_path = dest.join("traversal.tar");
    let mut f = std::fs::File::create(&archive_path).expect("create traversal.tar");
    let body = b"malicious content";

    let entries: &[&str] = &[
        "../../../etc/passwd",
        "/etc/shadow",
        "%2e%2e%2f%2e%2e%2fetc/passwd",
        "subdir/../../../etc/passwd",
    ];
    for entry in entries {
        f.write_all(&tar_file_header(entry, body.len() as u64)).expect("write header");
        f.write_all(body).expect("write body");
        f.write_all(&tar_padding(body.len() as u64)).expect("write padding");
    }
    f.write_all(&tar_end_marker()).expect("write end marker");
    archive_path
}

/// Build a tar archive containing a symlink pointing to an arbitrary target.
///
/// Uses `tar -cf` (without `-h`) to preserve the symlink in the archive rather
/// than following it. This is how a real attacker would craft a malicious archive.
///
/// # Examples
///
/// ```rust,no_run
/// let (_dir, tar) = fixtures::tar_with_symlink("evil_link", "/etc/passwd").unwrap();
/// ```
#[cfg(unix)]
#[allow(dead_code)]
pub fn tar_with_symlink(link_name: &str, link_target: &str) -> io::Result<(TempDir, PathBuf)> {
    let dir = TempDir::new()?;
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src)?;

    std::os::unix::fs::symlink(link_target, src.join(link_name))?;

    let tar_path = dir.path().join("symlink.tar");
    // Use -cf (not -chf) so the symlink itself is stored in the archive,
    // rather than the file it points to.
    let output = Command::new("tar")
        .args([
            "-cf",
            tar_path.to_str().expect("tar_path must be valid UTF-8"),
            "-C",
            src.to_str().expect("src path must be valid UTF-8"),
            ".",
        ])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "tar with symlink creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok((dir, tar_path))
}

/// Create a tar archive containing symlinks that escape the deployment directory.
///
/// Includes: absolute symlink to `/etc/passwd`, relative escape `../../etc/shadow`,
/// and a chained symlink pair.
pub fn tar_with_symlinks(dest: &Path) -> PathBuf {
    let archive_path = dest.join("symlinks.tar");
    let mut f = std::fs::File::create(&archive_path).expect("create symlinks.tar");

    f.write_all(&tar_symlink_header("link_abs", "/etc/passwd"))
        .expect("write symlink header");
    f.write_all(&tar_symlink_header("link_rel", "../../etc/shadow"))
        .expect("write symlink header");
    // Chained: step1 -> subdir, step2 -> step1/../../../etc/passwd
    f.write_all(&tar_symlink_header("chain_a", "subdir"))
        .expect("write symlink header");
    f.write_all(&tar_symlink_header("chain_b", "chain_a/../../../etc/passwd"))
        .expect("write symlink header");
    f.write_all(&tar_end_marker()).expect("write end marker");
    archive_path
}

/// Create a tar archive containing hardlinks to sensitive files.
pub fn tar_with_hardlinks(dest: &Path) -> PathBuf {
    let archive_path = dest.join("hardlinks.tar");
    let mut f = std::fs::File::create(&archive_path).expect("create hardlinks.tar");

    f.write_all(&tar_hardlink_header("hl_passwd", "/etc/passwd"))
        .expect("write hardlink header");
    f.write_all(&tar_hardlink_header("hl_shadow", "/etc/shadow"))
        .expect("write hardlink header");
    f.write_all(&tar_end_marker()).expect("write end marker");
    archive_path
}

/// Create a tar archive that acts as an archive bomb — a small file that
/// expands to a very large payload.
///
/// Generates a single entry claiming a 1 GiB size but backed by a small
/// body of repeated zeros. The header advertises the large size so that
/// extraction logic checking ratios can detect it without writing 1 GiB.
pub fn tar_archive_bomb(dest: &Path) -> PathBuf {
    let archive_path = dest.join("bomb.tar");
    let mut f = std::fs::File::create(&archive_path).expect("create bomb.tar");

    // Advertise 1 GiB in the header but only write a small body.
    // Extraction code should check the declared size against thresholds
    // before attempting to write.
    let declared_size: u64 = 1024 * 1024 * 1024; // 1 GiB
    f.write_all(&tar_file_header("bomb.bin", declared_size)).expect("write header");
    // Write only 4 KiB of actual data — enough for tests to inspect the
    // header without filling disk.
    let small_body = vec![0u8; 4096];
    f.write_all(&small_body).expect("write small body");
    f.write_all(&tar_padding(small_body.len() as u64)).expect("write padding");
    f.write_all(&tar_end_marker()).expect("write end marker");
    archive_path
}

/// Create a tar archive with a large number of small file entries to test
/// inode/file-count exhaustion limits.
///
/// Generates 10 000 entries (enough to trigger file-count limits without
/// being prohibitively slow in tests).
pub fn tar_with_millions_of_files(dest: &Path) -> PathBuf {
    let archive_path = dest.join("many_files.tar");
    let mut f = std::fs::File::create(&archive_path).expect("create many_files.tar");
    let body = b"x";

    for i in 0..10_000 {
        let name = format!("f/{:05}", i);
        f.write_all(&tar_file_header(&name, body.len() as u64)).expect("write header");
        f.write_all(body).expect("write body");
        f.write_all(&tar_padding(body.len() as u64)).expect("write padding");
    }
    f.write_all(&tar_end_marker()).expect("write end marker");
    archive_path
}

// ---------------------------------------------------------------------------
// AppSpec generators
// ---------------------------------------------------------------------------

/// Return a malformed AppSpec YAML string for the given variant.
///
/// Supported variants:
/// - `"missing_version"` — no `version` field
/// - `"invalid_hook"` — hook name not in the allowed set
/// - `"nonexistent_source"` — source file that does not exist on disk
pub fn malformed_appspec(variant: &str) -> String {
    match variant {
        "missing_version" => {
            "os: linux\nfiles:\n  - source: /src\n    destination: /dst\n".to_string()
        }
        "invalid_hook" => {
            "version: 0.0\nos: linux\nhooks:\n  NotARealHook:\n    - location: scripts/run.sh\n"
                .to_string()
        }
        "nonexistent_source" => {
            "version: 0.0\nos: linux\nfiles:\n  - source: /does_not_exist_xyz\n    destination: /dst\n"
                .to_string()
        }
        other => panic!("unknown malformed_appspec variant: {other}"),
    }
}

/// Return an AppSpec YAML string that requests a SUID or SGID permission mode.
///
/// `mode` should be an octal value like `0o4755`, `0o6755`, or `0o2755`.
pub fn appspec_with_suid(mode: u32) -> String {
    format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: /src\n    destination: /dst\n\
         permissions:\n  - object: /dst\n    mode: \"{:o}\"\n",
        mode
    )
}

/// Return an AppSpec YAML string that specifies a SELinux context.
///
/// Pass a dangerous context like `"unconfined_u:unconfined_r:unconfined_t:s0"`
/// or a valid one like `"system_u:object_r:httpd_sys_content_t:s0"`.
pub fn appspec_with_selinux(context: &str) -> String {
    format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: /src\n    destination: /dst\n\
         permissions:\n  - object: /dst\n    context:\n      user: placeholder\n      type: {context}\n",
    )
}

// ---------------------------------------------------------------------------
// State file generators
// ---------------------------------------------------------------------------

/// Return a corrupted state file payload for the given variant.
///
/// Supported variants:
/// - `"truncated_json"` — valid JSON prefix cut off mid-value
/// - `"invalid_utf8"` — bytes containing invalid UTF-8 sequences
/// - `"random_bytes"` — completely random-looking data
pub fn corrupted_state_file(variant: &str) -> Vec<u8> {
    match variant {
        "truncated_json" => br#"{"deployment_id": "d-ABC12345", "sta"#.to_vec(),
        "invalid_utf8" => {
            let mut data = br#"{"deployment_id": ""#.to_vec();
            data.extend_from_slice(&[0xFF, 0xFE, 0x80, 0xC0]);
            data.extend_from_slice(br#""}"#);
            data
        },
        "random_bytes" => {
            // Deterministic "random-looking" bytes for reproducible tests.
            (0u8..=255).collect()
        },
        other => panic!("unknown corrupted_state_file variant: {other}"),
    }
}

/// Return a deployment ID containing path traversal sequences.
pub fn deployment_id_with_traversal() -> String {
    "../../../etc/passwd".to_string()
}

// ---------------------------------------------------------------------------
// Self-tests — verify generators produce valid output
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_traversal_archive_is_nonempty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tar_with_path_traversal(dir.path());
        let meta = std::fs::metadata(&path).expect("metadata");
        assert!(meta.len() > 1024, "archive should contain multiple entries");
    }

    #[test]
    fn symlink_archive_is_nonempty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tar_with_symlinks(dir.path());
        let meta = std::fs::metadata(&path).expect("metadata");
        assert!(meta.len() > 512, "archive should contain symlink entries");
    }

    #[test]
    fn hardlink_archive_is_nonempty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tar_with_hardlinks(dir.path());
        let meta = std::fs::metadata(&path).expect("metadata");
        assert!(meta.len() > 512, "archive should contain hardlink entries");
    }

    #[test]
    fn archive_bomb_header_declares_large_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tar_archive_bomb(dir.path());
        let data = std::fs::read(&path).expect("read");
        // The size field is at bytes 124..135 in the first header.
        let size_field = std::str::from_utf8(&data[124..135]).expect("utf8");
        let declared = u64::from_str_radix(size_field.trim(), 8).expect("parse octal");
        assert_eq!(declared, 1024 * 1024 * 1024, "header should declare 1 GiB");
    }

    #[test]
    fn many_files_archive_has_expected_entry_count() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = tar_with_millions_of_files(dir.path());
        let data = std::fs::read(&path).expect("read");
        // Each entry = 512 header + 1 byte body + 511 padding = 1024 bytes
        // Plus 1024 end marker. 10_000 * 1024 + 1024 = 10_241_024
        assert_eq!(data.len(), 10_000 * 1024 + 1024);
    }

    #[test]
    fn malformed_appspec_missing_version_has_no_version_key() {
        let yaml = malformed_appspec("missing_version");
        assert!(!yaml.contains("version:"), "should not contain version field");
    }

    #[test]
    fn appspec_with_suid_contains_mode() {
        let yaml = appspec_with_suid(0o4755);
        assert!(yaml.contains("4755"), "should contain octal mode 4755");
    }

    #[test]
    fn appspec_with_selinux_contains_context() {
        let yaml = appspec_with_selinux("unconfined_t");
        assert!(yaml.contains("unconfined_t"));
    }

    #[test]
    fn corrupted_state_truncated_is_invalid_json() {
        let data = corrupted_state_file("truncated_json");
        let result = serde_json::from_slice::<serde_json::Value>(&data);
        assert!(result.is_err(), "truncated JSON should fail to parse");
    }

    #[test]
    fn corrupted_state_invalid_utf8_contains_bad_bytes() {
        let data = corrupted_state_file("invalid_utf8");
        assert!(std::str::from_utf8(&data).is_err(), "should contain invalid UTF-8");
    }

    #[test]
    fn deployment_id_traversal_contains_dot_dot() {
        let id = deployment_id_with_traversal();
        assert!(id.contains(".."), "should contain path traversal");
    }
}
