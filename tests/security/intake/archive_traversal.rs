//! CR 1b — Archive & Path Traversal security tests (TO-006, TO-009).
//!
//! Validates that the bundle unpacker (`host_command::bundle_unpacker`) defends
//! against path traversal, symlink, and hardlink attacks in deployment archives.
//!
//! These tests exercise the *real* `unpack()` function against adversarial
//! archives built at runtime by the fixtures in `security::fixtures`.
//!
//! Security properties tested:
//! - No extracted file may escape the deployment directory (TO-006)
//! - Archives containing symlinks pointing outside the deployment dir are rejected (TO-009)
//! - Archives containing hardlinks to sensitive files are rejected (TO-009)
//!
//! All tests are `#[ignore]` because `bundle_unpacker::unpack()` currently delegates
//! to system `tar`/`unzip` without pre- or post-extraction validation.

use aws_codedeploy_agent::host_command::bundle_unpacker;
use tempfile::TempDir;

use crate::security::fixtures;

// ---------------------------------------------------------------------------
// TO-006: Path Traversal
// ---------------------------------------------------------------------------

/// TO-006 / TC-006-01: Path traversal archives must not escape deployment dir.
///
/// Security property: when an archive contains entries with `../` sequences
/// (e.g. `../../../etc/passwd`), the unpacker must either reject the archive
/// entirely or strip/sanitize the path so that no file is written outside the
/// destination directory.
///
/// This is the primary path-traversal attack surface — a compromised or
/// malicious deployment bundle could overwrite arbitrary files on the host.
#[test]
#[ignore] // TODO: enable after implementing path validation in unpack()
fn unpack_rejects_path_traversal() {
    let traversal_paths = ["../../../etc/passwd", "../../root/.ssh/authorized_keys"];

    for traversal in &traversal_paths {
        let (_fixture_dir, tar_path) = fixtures::tar_with_traversal(traversal, b"pwned")
            .expect("fixture tar_with_traversal must succeed");

        let dest_dir = TempDir::new().unwrap();
        let dest = dest_dir.path().join("deployment");

        let result = bundle_unpacker::unpack(&tar_path, &dest, "tar");

        // The unpacker must reject the archive
        assert!(result.is_err(), "Extraction must fail for traversal path: {traversal}");

        // Even if extraction were attempted, no files should escape
        let escaped_passwd = dest_dir.path().join("etc/passwd");
        assert!(
            !escaped_passwd.exists(),
            "File must not be written outside deployment dir for: {traversal}"
        );

        let escaped_ssh = dest_dir.path().join("root/.ssh/authorized_keys");
        assert!(
            !escaped_ssh.exists(),
            "File must not be written outside deployment dir for: {traversal}"
        );
    }
}

/// TO-006 / TC-006-02: URL-encoded path traversal must be detected.
///
/// Security property: path components must be URL-decoded before validation.
/// An attacker may URL-encode `../` as `..%2F` or `%2e%2e%2f` to bypass
/// naive string checks that only look for literal `..` sequences.
///
/// This test validates that the decode + canonicalize step catches these
/// encoded traversal attempts. System `tar` may not create URL-encoded
/// filenames, so this tests the *validation layer* rather than real archives.
#[test]
#[ignore] // TODO: enable after implementing URL-decode + path validation
fn unpack_rejects_url_encoded_traversal() {
    let encoded_paths = [
        ("..%2F..%2Fetc/passwd", "..%2F traversal"),
        ("%2e%2e%2f%2e%2e%2f", "full percent-encoded traversal"),
        ("..%5C..%5Cetc/passwd", "backslash-encoded traversal"),
    ];

    for (encoded, description) in &encoded_paths {
        // URL-decode and verify the decoded form contains traversal patterns
        let decoded = percent_decode(encoded);
        assert!(
            decoded.contains(".."),
            "URL-decoded path should contain traversal for {description}: {encoded} -> {decoded}"
        );
    }

    // Also test with a real archive whose filename contains a literal `%2e%2e`
    // (some servers may double-encode). The unpacker should still reject.
    let (_fixture_dir, tar_path) = fixtures::tar_with_traversal("..%2F..%2Fetc/passwd", b"pwned")
        .expect("fixture tar_with_traversal must succeed");

    let dest_dir = TempDir::new().unwrap();
    let dest = dest_dir.path().join("deployment");

    let result = bundle_unpacker::unpack(&tar_path, &dest, "tar");

    // Even though the raw filename is `..%2F..%2Fetc/passwd`, validation
    // should decode first, detect traversal, and reject.
    assert!(result.is_err(), "Extraction must fail for URL-encoded traversal");
}

/// TO-006 / TC-006-03: Canonical path resolution must catch disguised traversal.
///
/// Security property: even when the traversal is hidden inside intermediate
/// path components (e.g. `subdir/../../../etc/passwd`), the *canonical* path
/// of every extracted entry must be verified to be under the deployment
/// directory. A naïve check for `../` at the start of the path is insufficient.
#[test]
#[ignore] // TODO: enable after implementing canonical path validation in unpack()
fn unpack_rejects_canonicalized_traversal() {
    // This traversal disguises itself by starting with a legitimate subdirectory
    // and then escaping via enough `..` components.
    let disguised_paths = [
        "subdir/../../../etc/passwd",
        "a/b/c/../../../../etc/shadow",
        "legit/./../../etc/hostname",
    ];

    for disguised in &disguised_paths {
        let (_fixture_dir, tar_path) = fixtures::tar_with_traversal(disguised, b"pwned")
            .expect("fixture tar_with_traversal must succeed");

        let dest_dir = TempDir::new().unwrap();
        let dest = dest_dir.path().join("deployment");

        let result = bundle_unpacker::unpack(&tar_path, &dest, "tar");

        assert!(result.is_err(), "Extraction must fail for canonicalized traversal: {disguised}");

        // Verify nothing escaped
        let escaped = dest_dir.path().join("etc");
        assert!(!escaped.exists(), "No files must escape deployment dir via: {disguised}");
    }
}

// ---------------------------------------------------------------------------
// TO-009: Symlink & Hardlink Attacks
// ---------------------------------------------------------------------------

/// TO-009 / TC-009-01: Symlinks in archives must be detected and rejected.
///
/// Security property: when an archive contains a symbolic link whose target
/// is outside the deployment directory (e.g. `/etc/passwd`, `/root`), the
/// unpacker must refuse to create the symlink. Otherwise an attacker can
/// read or overwrite arbitrary files through the symlink.
#[cfg(unix)]
#[test]
#[ignore] // TODO: enable after implementing symlink detection in unpack()
fn unpack_rejects_symlinks() {
    let symlink_targets = [
        ("etc_passwd_link", "/etc/passwd"),
        ("root_link", "/root"),
        ("relative_escape", "../../../../etc/shadow"),
    ];

    for (link_name, link_target) in &symlink_targets {
        let (_fixture_dir, tar_path) = fixtures::tar_with_symlink(link_name, link_target)
            .expect("fixture tar_with_symlink must succeed");

        let dest_dir = TempDir::new().unwrap();
        let dest = dest_dir.path().join("deployment");

        let result = bundle_unpacker::unpack(&tar_path, &dest, "tar");

        assert!(
            result.is_err(),
            "Extraction must fail when archive contains symlink {link_name} -> {link_target}"
        );

        // The symlink must not have been created in the deployment directory
        let link_path = dest.join(link_name);
        assert!(
            !link_path.exists() && !link_path.is_symlink(),
            "Symlink must not be created in deployment directory: {link_name} -> {link_target}"
        );
    }
}

/// TO-009 / TC-009-02: Hardlinks in archives must be detected.
///
/// Security property: tar hardlinks can reference files outside the archive,
/// allowing an attacker to read the contents of sensitive host files after
/// extraction (e.g. `/etc/shadow`). The unpacker must inspect entry types
/// and reject any `EntryType::Link` whose target resolves outside the
/// deployment directory.
///
/// Implementation note: this requires either pre-scanning with `tar --list`
/// or switching to a Rust tar crate that provides entry-type inspection.
#[cfg(unix)]
#[test]
#[ignore] // TODO: enable after implementing hardlink detection in unpack()
fn unpack_rejects_hardlinks_to_sensitive_files() {
    // Build an archive containing a hardlink to a file outside the archive.
    // GNU tar supports the `--add-file` trick, but creating cross-device
    // hardlinks is restricted by the kernel, so we test with an in-device link.
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    // Create a normal file, then hardlink to it from a different name.
    // In a real attack the hardlink target would be outside the deployment dir;
    // here we verify the unpacker inspects entry types at all.
    let original = src.join("secret.txt");
    std::fs::write(&original, "sensitive-data").unwrap();
    std::fs::hard_link(&original, src.join("hardlink_to_secret")).unwrap();

    let tar_path = dir.path().join("hardlink.tar");
    let output = std::process::Command::new("tar")
        .args([
            "-cf",
            tar_path.to_str().unwrap(),
            "-C",
            src.to_str().unwrap(),
            ".",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "tar creation must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let dest_dir = TempDir::new().unwrap();
    let dest = dest_dir.path().join("deployment");

    let result = bundle_unpacker::unpack(&tar_path, &dest, "tar");

    // The unpacker should detect the hardlink entry and reject.
    // Note: this test currently validates that the infrastructure to *detect*
    // hardlink entries exists. A production fix would use the `tar` Rust crate
    // to iterate entries and reject `EntryType::Link`.
    assert!(result.is_err(), "Extraction should fail when archive contains hardlinks");
}

/// TO-009 / TC-009-03: TOCTOU symlink race condition (code-review only).
///
/// Security property: a concurrent process must not be able to create a
/// symlink between the time the unpacker validates a path and the time it
/// writes data. This requires atomic filesystem operations (e.g. `O_NOFOLLOW`
/// flags, opening the parent directory first, then using `openat` or
/// `linkat`).
///
/// This race condition is not reliably testable in a unit test because it
/// depends on precise timing of concurrent filesystem operations. It is
/// documented here as a code-review checklist item:
///
/// **Code Review Checklist:**
/// - [ ] Extraction uses `O_NOFOLLOW` or equivalent when creating files
/// - [ ] Parent directory is opened with `O_DIRECTORY` before child creation
/// - [ ] `openat(2)` / `mkdirat(2)` used instead of full-path operations
/// - [ ] No TOCTOU gap between path validation and file write
#[test]
#[ignore] // Code-review only — TOCTOU races are not deterministically testable
fn toctou_symlink_race_is_mitigated() {
    // This test intentionally has no assertions. It exists solely as a
    // marker for the code-review checklist above. Run with --include-ignored
    // to verify the test compiles but expect no meaningful runtime behavior.
    //
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Minimal percent-decoding for URL-encoded path components.
///
/// Handles `%XX` hex sequences. This is intentionally simple — production code
/// should use a proper URL decoding library.
fn percent_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    result.push(byte as char);
                    continue;
                }
            }
            // Malformed %XX — pass through
            result.push('%');
            result.push_str(&hex);
        } else {
            result.push(c);
        }
    }
    result
}

#[cfg(test)]
mod percent_decode_tests {
    use super::percent_decode;

    #[test]
    fn decodes_dot_dot_slash() {
        assert_eq!(percent_decode("..%2F..%2Fetc/passwd"), "../../etc/passwd");
    }

    #[test]
    fn decodes_full_encoded_dots() {
        let decoded = percent_decode("%2e%2e%2f%2e%2e%2f");
        assert_eq!(decoded, "../../");
    }

    #[test]
    fn decodes_backslash_encoded() {
        assert_eq!(percent_decode("..%5C..%5Cetc/passwd"), "..\\..\\etc/passwd");
    }

    #[test]
    fn passthrough_no_encoding() {
        assert_eq!(percent_decode("normal/path.txt"), "normal/path.txt");
    }
}
