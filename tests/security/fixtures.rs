//! Security test fixtures — generates malicious inputs for testing.
//!
//! All fixtures are generated at runtime, never checked into the repository.
//! This avoids accidentally distributing weaponized test data.
//!
//! # Convention
//!
//! Every builder returns `io::Result<(TempDir, PathBuf)>` where `TempDir` owns
//! the temporary directory (keeping it alive for the test's scope) and `PathBuf`
//! is the path to the generated artifact inside it.
//!
//! Some fixtures will not be used until later CRs add tests that call them.
#![allow(dead_code)]

use std::io;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Archive fixtures — tar files with adversarial entries
// ---------------------------------------------------------------------------

/// Build a tar archive containing a file with a path-traversal entry.
///
/// Creates `<tmp>/evil/<traversal_path>` then tars it relative to `evil/`.
/// The resulting archive will attempt to escape the extraction directory
/// when unpacked.
///
/// # Examples
///
/// ```rust,no_run
/// let (_dir, tar) = fixtures::tar_with_traversal("../../../etc/passwd", b"pwned").unwrap();
/// ```
pub fn tar_with_traversal(traversal_path: &str, content: &[u8]) -> io::Result<(TempDir, PathBuf)> {
    let dir = TempDir::new()?;
    let evil_dir = dir.path().join("evil");
    let target = evil_dir.join(traversal_path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, content)?;

    let tar_path = dir.path().join("malicious.tar");
    let output = Command::new("tar")
        .args([
            "-cf",
            tar_path.to_str().unwrap(),
            "-C",
            evil_dir.to_str().unwrap(),
            ".",
        ])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "tar creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok((dir, tar_path))
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
            tar_path.to_str().unwrap(),
            "-C",
            src.to_str().unwrap(),
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

/// Build a tar archive containing a binary with SUID permissions.
///
/// Creates a file with mode `target_mode` (e.g., `0o4755` for SUID), tars it,
/// and returns the archive path. Used to test that the unpacker strips
/// SUID/SGID bits after extraction.
#[cfg(unix)]
pub fn tar_with_suid_binary(target_mode: u32) -> io::Result<(TempDir, PathBuf)> {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new()?;
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src)?;

    let binary = src.join("setuid_binary");
    std::fs::write(&binary, "#!/bin/sh\necho test")?;
    let mut perms = std::fs::metadata(&binary)?.permissions();
    perms.set_mode(target_mode);
    std::fs::set_permissions(&binary, perms)?;

    let tar_path = dir.path().join("suid.tar");
    let output = Command::new("tar")
        .args([
            "-cf",
            tar_path.to_str().unwrap(),
            "-C",
            src.to_str().unwrap(),
            ".",
        ])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "tar with suid creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok((dir, tar_path))
}

/// Build an empty tar archive (0 bytes) for boundary testing.
pub fn empty_archive() -> io::Result<(TempDir, PathBuf)> {
    let dir = TempDir::new()?;
    let empty_tar = dir.path().join("empty.tar");
    std::fs::write(&empty_tar, b"")?;
    Ok((dir, empty_tar))
}

// ---------------------------------------------------------------------------
// AppSpec YAML fixtures — generates malicious or edge-case AppSpec strings
// ---------------------------------------------------------------------------

/// Generate an AppSpec YAML string with SUID/SGID permission modes.
///
/// The `mode` parameter should be an octal string like `"4755"`, `"6755"`, etc.
pub fn appspec_with_suid_mode(mode: &str) -> String {
    format!("version: 0.0\nos: linux\npermissions:\n  - object: /app\n    mode: \"{mode}\"\n")
}

/// Generate an AppSpec with a specific SELinux context type.
///
/// Used to test both dangerous types (`unconfined_t`) and safe ones (`httpd_sys_content_t`).
pub fn appspec_with_selinux_type(type_: &str) -> String {
    format!(
        "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: {type_}\n"
    )
}

/// Generate an AppSpec with a full SELinux context (user + type + range).
pub fn appspec_with_full_selinux_context(user: &str, type_: &str, range: &str) -> String {
    format!(
        "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      name: {user}\n      type: {type_}\n      range: {range}\n"
    )
}

/// Generate a completely malformed AppSpec.
///
/// Variants:
/// - `"missing_version"` — omits the version field
/// - `"missing_os"` — omits the os field
/// - `"missing_source"` — file mapping without source
/// - `"missing_destination"` — file mapping without destination
/// - `"empty_location"` — hook with missing location
/// - `"invalid_yaml"` — syntactically invalid YAML
/// - `"empty"` — empty string
/// - `"whitespace"` — whitespace-only
/// - `"windows_permissions"` — permissions section on Windows OS
pub fn appspec_malformed(variant: &str) -> String {
    match variant {
        "missing_version" => "os: linux\n".to_string(),
        "missing_os" => "version: 0.0\n".to_string(),
        "missing_source" => "version: 0.0\nos: linux\nfiles:\n  - destination: /dest\n".to_string(),
        "missing_destination" => "version: 0.0\nos: linux\nfiles:\n  - source: /src\n".to_string(),
        "empty_location" => {
            "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - timeout: 30\n".to_string()
        },
        "invalid_yaml" => "{{{invalid".to_string(),
        "empty" => String::new(),
        "whitespace" => "   \n  \n  ".to_string(),
        "windows_permissions" => {
            "version: 0.0\nos: windows\npermissions:\n  - object: /tmp\n    mode: \"0755\"\n"
                .to_string()
        },
        _ => "{{{unknown variant}}".to_string(),
    }
}

/// Generate a minimal valid AppSpec.
pub fn appspec_minimal_valid() -> String {
    "version: 0.0\nos: linux\n".to_string()
}

/// Generate a valid AppSpec with files and hooks.
pub fn appspec_with_files_and_hooks() -> String {
    "version: 0.0\nos: linux\nfiles:\n  - source: /src\n    destination: /dest\nhooks:\n  ApplicationStart:\n    - location: scripts/start.sh\n      timeout: 300\n".to_string()
}

// ---------------------------------------------------------------------------
// Config YAML fixtures — generates edge-case agent configuration strings
// ---------------------------------------------------------------------------

/// Generate a config YAML with a specific endpoint value.
pub fn config_with_endpoint(endpoint: &str) -> String {
    format!("deploy_control_endpoint: \"{endpoint}\"\n")
}

/// Generate a config YAML with a specific timeout value.
pub fn config_with_timeout(timeout: u64) -> String {
    format!("kill_agent_max_wait_time_seconds: {timeout}\n")
}

/// Generate a minimal valid config YAML.
pub fn config_minimal_valid() -> String {
    "wait_between_runs: 30\n".to_string()
}
