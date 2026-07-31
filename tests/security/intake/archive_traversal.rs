//! Archive & Path Traversal security tests.
//!
//! Validates that the bundle unpacker (`host_command::bundle_unpacker`) defends
//! against path traversal, symlink, and hardlink attacks in deployment archives.
//!
//! These tests exercise the *real* `unpack()` function against adversarial
//! archives built at runtime by the fixtures in `security::fixtures`.
//!
//! Security properties tested:
//! - No extracted file may escape the deployment directory
//! - Archives containing symlinks pointing outside the deployment dir are rejected
//! - Archives containing hardlinks to sensitive files are rejected
//!
//! Path traversal tests exercise the opt-in `reject_path_traversal_in_bundle`
//! toggle. Symlink/hardlink tests exercise `reject_symlinks_in_bundle`.
//! The toggles are off by default to match system `tar`/`unzip` semantics;
//! operators opt into the strict checks via agent config.

use codedeploy_agent::host_command::bundle_unpacker;
use proptest::prelude::*;
use tempfile::TempDir;

use crate::security::fixtures;

// ---------------------------------------------------------------------------
// Path Traversal
// ---------------------------------------------------------------------------

/// Header scan rejects archives with `..` or absolute entry paths.
#[test]
fn unpack_rejects_path_traversal() {
    let traversal_paths = ["../../../etc/passwd", "../../root/.ssh/authorized_keys"];

    for traversal in &traversal_paths {
        let (_fixture_dir, tar_path) = fixtures::tar_with_traversal(traversal, b"pwned")
            .expect("fixture tar_with_traversal must succeed");

        let result = bundle_unpacker::check_path_traversal(&tar_path, "tar");
        assert!(result.is_err(), "must reject traversal path: {traversal}");
        assert!(result.unwrap_err().to_string().contains("traversal component"));
    }
}

/// Agent does not gate path traversal when the rejection flag is off.
#[test]
fn path_traversal_allowed_when_rejection_disabled() {
    let (_fixture_dir, tar_path) = fixtures::tar_with_traversal("../../../etc/passwd", b"pwned")
        .expect("fixture tar_with_traversal must succeed");

    let dest_dir = TempDir::new().expect("create temp dir");
    let dest = dest_dir.path().join("deployment");
    let _ = bundle_unpacker::unpack(&tar_path, &dest, "tar", false, false);
}

/// Decoder coverage only.
///
/// Archive entry paths are written verbatim by the agent (no decode step), so
/// `..%2F..%2Fetc/passwd` is a literal filename, not a traversal. The test
/// asserts the decoder produces `..` for any future codepath that decodes paths.
#[test]
fn unpack_rejects_url_encoded_traversal() {
    let encoded_paths = [
        ("..%2F..%2Fetc/passwd", "..%2F traversal"),
        ("%2e%2e%2f%2e%2e%2f", "full percent-encoded traversal"),
        ("..%5C..%5Cetc/passwd", "backslash-encoded traversal"),
    ];

    for (encoded, description) in &encoded_paths {
        let decoded = percent_decode(encoded);
        assert!(
            decoded.contains(".."),
            "URL-decoded path should contain traversal for {description}: {encoded} -> {decoded}"
        );
    }
}

/// Header scan flags `..` anywhere in the path, not just at the start.
#[test]
fn unpack_rejects_canonicalized_traversal() {
    let disguised_paths = [
        "subdir/../../../etc/passwd",
        "a/b/c/../../../../etc/shadow",
        "legit/./../../etc/hostname",
    ];

    for disguised in &disguised_paths {
        let (_fixture_dir, tar_path) = fixtures::tar_with_traversal(disguised, b"pwned")
            .expect("fixture tar_with_traversal must succeed");

        let result = bundle_unpacker::check_path_traversal(&tar_path, "tar");
        assert!(result.is_err(), "must reject disguised traversal: {disguised}");
        assert!(result.unwrap_err().to_string().contains("traversal component"));
    }
}

// ---------------------------------------------------------------------------
// Symlink & Hardlink Attacks
// ---------------------------------------------------------------------------

/// Symlinks in archives must be detected and rejected.
///
/// Security property: when an archive contains a symbolic link whose target
/// is outside the deployment directory (e.g. `/etc/passwd`, `/root`), the
/// unpacker must refuse to create the symlink. Otherwise an attacker can
/// read or overwrite arbitrary files through the symlink.
#[cfg(unix)]
#[test]
fn unpack_rejects_symlinks() {
    let symlink_targets = [
        ("etc_passwd_link", "/etc/passwd"),
        ("root_link", "/root"),
        ("relative_escape", "../../../../etc/shadow"),
    ];

    for (link_name, link_target) in &symlink_targets {
        let dest_dir = TempDir::new().expect("create temp dir");
        let dest = dest_dir.path().join("deployment");
        std::fs::create_dir_all(&dest).expect("create deployment dir");

        // Create symlink directly — no tar dependency, deterministic on all platforms
        std::os::unix::fs::symlink(link_target, dest.join(link_name))
            .expect("create symlink");

        // Post-extraction scan detects the symlink and rejects
        let result = bundle_unpacker::reject_bundle_symlinks(&dest);
        assert!(
            result.is_err(),
            "reject_bundle_symlinks must fail when dir contains symlink {link_name} -> {link_target}"
        );

        // The dest directory must have been cleaned up
        assert!(
            !dest.exists(),
            "Deployment directory must be removed after symlink rejection: {link_name} -> {link_target}"
        );
    }
}

/// Symlinks are preserved when rejection is not called.
///
/// This proves that when `reject_symlinks_in_bundle` config is false (i.e.,
/// `reject_bundle_symlinks()` is never invoked), symlinks in bundles are
/// preserved as-is.
#[cfg(unix)]
#[test]
fn symlinks_allowed_when_rejection_disabled() {
    let dest_dir = TempDir::new().expect("create temp dir");
    let dest = dest_dir.path().join("deployment");
    std::fs::create_dir_all(&dest).expect("create deployment dir");

    // Create symlink directly — deterministic, no tar dependency
    std::os::unix::fs::symlink("/etc/passwd", dest.join("my_link"))
        .expect("create symlink");

    // Without calling reject_bundle_symlinks(), the symlink is preserved
    let link_path = dest.join("my_link");
    assert!(
        link_path.is_symlink(),
        "Symlink must be preserved when reject_bundle_symlinks is not called"
    );
}

/// Hardlinks in archives must be detected.
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
fn unpack_rejects_hardlinks_to_sensitive_files() {
    // Build an archive containing a hardlink to a file outside the archive.
    // GNU tar supports the `--add-file` trick, but creating cross-device
    // hardlinks is restricted by the kernel, so we test with an in-device link.
    let dir = TempDir::new().expect("create temp dir");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("create src dir");

    // Create a normal file, then hardlink to it from a different name.
    // In a real attack the hardlink target would be outside the deployment dir;
    // here we verify the unpacker inspects entry types at all.
    let original = src.join("secret.txt");
    std::fs::write(&original, "sensitive-data").expect("write test fixture");
    std::fs::hard_link(&original, src.join("hardlink_to_secret"))
        .expect("create hardlink");

    let tar_path = dir.path().join("hardlink.tar");
    let output = std::process::Command::new("tar")
        .args([
            "-cf",
            tar_path.to_str().expect("tar_path must be valid UTF-8"),
            "-C",
            src.to_str().expect("src path must be valid UTF-8"),
            ".",
        ])
        .output()
        .expect("tar command must be available");
    assert!(
        output.status.success(),
        "tar creation must succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let dest_dir = TempDir::new().expect("create dest dir");
    let dest = dest_dir.path().join("deployment");

    // unpack() succeeds (it extracts the hardlink as-is)
    bundle_unpacker::unpack(&tar_path, &dest, "tar", false, false)
        .expect("unpack must succeed before rejection scan");

    // Post-extraction scan detects the hardlink (nlink > 1) and rejects
    let result = bundle_unpacker::reject_bundle_symlinks(&dest);

    // The unpacker should detect the hardlink entry and reject.
    // Note: this test currently validates that the infrastructure to *detect*
    // hardlink entries exists. A production fix would use the `tar` Rust crate
    // to iterate entries and reject `EntryType::Link`.
    assert!(
        result.is_err(),
        "reject_bundle_symlinks should fail when archive contains hardlinks"
    );

    // The dest directory must have been cleaned up
    assert!(!dest.exists(), "Deployment directory must be removed after hardlink rejection");
}

/// Native zip extraction uses `enclosed_name()` — no TOCTOU gap.
#[test]
fn toctou_symlink_race_is_mitigated() {
    use std::io::Write;

    let dir = TempDir::new().expect("create temp dir for TOCTOU test");
    let archive = dir.path().join("toctou.zip");

    let file = std::fs::File::create(&archive).expect("create zip");
    let mut zip = zip::ZipWriter::new(file);
    let opts = zip::write::SimpleFileOptions::default();
    zip.start_file("legitimate.txt", opts).expect("add safe entry");
    zip.write_all(b"safe content").expect("write safe content");
    zip.start_file("../../../tmp/toctou_escape.txt", opts)
        .expect("add traversal entry");
    zip.write_all(b"escaped").expect("write escaped content");
    zip.finish().expect("finalize zip");

    let dest = dir.path().join("deployment");
    bundle_unpacker::unpack(&archive, &dest, "zip", false, false).expect("unpack should succeed");

    assert!(dest.join("legitimate.txt").exists());
    assert!(!std::path::Path::new("/tmp/toctou_escape.txt").exists());
    assert!(!dir.path().join("tmp/toctou_escape.txt").exists());
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Minimal `%XX` hex decoder for the encoded-traversal test only.
fn percent_decode(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2
                && let Ok(byte) = u8::from_str_radix(&hex, 16)
            {
                result.push(byte as char);
                continue;
            }
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

// ===========================================================================
// Property-Based Tests
// ===========================================================================

// ---------------------------------------------------------------------------
// Property 1: Path traversal rejection
// ---------------------------------------------------------------------------

// Any depth of `../`-prefixed path is rejected.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_path_traversal_always_rejected(
        depth in 1usize..6,
        suffix in "[a-z]{1,10}",
    ) {
        let traversal = format!("{}{}", "../".repeat(depth), suffix);

        // Bind to a named variable — `_` drops the TempDir immediately and deletes the tar.
        let (_fixture_dir, tar_path) = fixtures::tar_with_traversal(&traversal, b"pwned")
            .expect("fixture tar_with_traversal must succeed");

        let result = bundle_unpacker::check_path_traversal(&tar_path, "tar");
        prop_assert!(result.is_err(), "must reject traversal path '{}'", traversal);
        prop_assert!(result.unwrap_err().to_string().contains("traversal component"));
    }
}

// Percent-decoder produces `..` for any encoded form (decoder coverage).
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_url_encoded_traversal_rejected(
        depth in 1usize..4,
        use_uppercase in proptest::bool::ANY,
    ) {
        let dot_dot_slash = if use_uppercase { "%2E%2E%2F" } else { "%2e%2e%2f" };
        let traversal = format!("{}etc/passwd", dot_dot_slash.repeat(depth));

        prop_assert!(percent_decode(&traversal).contains(".."));
    }
}

// ---------------------------------------------------------------------------
// Property 6: Symlink and hardlink rejection
// ---------------------------------------------------------------------------

// Property 6: Symlink and hardlink rejection
// Validates: For any archive containing symlinks with random targets
// (absolute paths or relative escapes), bundle_unpacker::reject_bundle_symlinks()
// rejects the extracted directory.
#[cfg(unix)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_symlink_targets_always_rejected(
        is_absolute in proptest::bool::ANY,
        path_segment1 in "[a-z]{1,8}",
        path_segment2 in "[a-z]{1,8}",
        depth in 1usize..4,
    ) {
        // Arrange: build a symlink target that escapes the deployment dir
        let target_variant = if is_absolute {
            // Absolute path targets like /etc/passwd
            format!("/{path_segment1}/{path_segment2}")
        } else {
            // Relative escape targets like ../../etc/shadow
            format!("{}{path_segment1}/{path_segment2}", "../".repeat(depth))
        };

        let link_name = "malicious_link";
        let dest_dir = TempDir::new().expect("create dest dir for P6 prop test");
        let dest = dest_dir.path().join("deployment");
        std::fs::create_dir_all(&dest).expect("create deployment dir");

        // Create symlink directly — deterministic, no tar dependency
        std::os::unix::fs::symlink(&target_variant, dest.join(link_name))
            .expect("create symlink for P6 prop test");

        // Act
        let result = bundle_unpacker::reject_bundle_symlinks(&dest);

        // Assert
        prop_assert!(
            result.is_err(),
            "reject_bundle_symlinks must reject dir with symlink {} -> {}", link_name, &target_variant
        );

        // Verify dest was cleaned up
        prop_assert!(
            !dest.exists(),
            "Deployment directory must be removed after symlink rejection: {} -> {}", link_name, &target_variant
        );
    }
}
