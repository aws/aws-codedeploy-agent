//! Security tests for installer permission handling.
//!
//! - SUID/SGID rejection in extracted files and AppSpec modes
//! - SELinux context blocklist (`unconfined_t`, `kernel_t`, `init_t`)

// ---------------------------------------------------------------------------
// SUID/SGID Bit Handling
// ---------------------------------------------------------------------------

/// Extracted files must not carry SUID/SGID bits.
#[test]
fn extracted_files_have_no_suid_sgid() {
    if nix::unistd::Uid::effective().is_root() {
        // GNU tar honors SUID/SGID bits from archive headers only when run
        // as the superuser, so the strip this test asserts on happens only
        // for non-root extraction. Root-preserving extraction is the
        // documented default; reject_unsafe_permissions_in_bundle is the
        // opt-in control for it (covered by other tests in this module).
        return;
    }
    use codedeploy_agent::host_command::bundle_unpacker;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("create temp dir");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("create src dir");

    // Create files with SUID, SGID, and combined SUID+SGID bits
    let test_cases: &[(&str, u32)] = &[
        ("suid_binary", 0o4755), // SUID only
        ("sgid_binary", 0o2755), // SGID only
        ("both_binary", 0o6755), // SUID + SGID
    ];

    for (name, mode) in test_cases {
        let binary = src.join(name);
        std::fs::write(&binary, "#!/bin/sh\necho pwned").expect("write test binary");
        let mut perms = std::fs::metadata(&binary).expect("read binary metadata").permissions();
        perms.set_mode(*mode);
        std::fs::set_permissions(&binary, perms).expect("set binary permissions");
    }

    // Create a tar archive containing all three files
    let tar_path = dir.path().join("suid.tar");
    let output = Command::new("tar")
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
        "tar creation failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Extract using the production unpacker
    let dest = dir.path().join("deployment");
    bundle_unpacker::unpack(&tar_path, &dest, "tar", false, false)
        .expect("unpack() should succeed");

    // Verify SUID/SGID bits are stripped from every extracted file
    for (name, original_mode) in test_cases {
        let extracted = dest.join(name);
        assert!(extracted.exists(), "Expected extracted file: {name}");
        let mode = std::fs::metadata(&extracted)
            .expect("read extracted file metadata")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o6000,
            0,
            "SUID/SGID bits must be stripped after extraction: \
             file={name}, original={original_mode:04o}, extracted={mode:04o}"
        );
    }
}

/// `ChangeModeCommand` rejects SUID/SGID modes when the toggle is on.
#[test]
fn change_mode_strips_suid_sgid() {
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeModeCommand;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::NamedTempFile;

    let file = NamedTempFile::new().expect("failed to create temp file");
    let path = file.path().to_path_buf();

    // Modes with SUID, SGID, and combined bits — all must be rejected.
    let dangerous_modes = ["4755", "2755", "6755", "7777"];

    for mode_str in &dangerous_modes {
        let cmd = ChangeModeCommand::new(path.clone(), (*mode_str).to_string(), true, false);
        let mut cleanup: Vec<u8> = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(
            matches!(result, Err(InstallerError::UnsafePermissionRejected { .. })),
            "ChangeModeCommand must reject mode {mode_str} when reject_unsafe_permissions_in_bundle is on, got {result:?}"
        );

        // The file's mode must NOT have been changed to a SUID/SGID mode —
        // rejection should fail before fs::set_permissions runs.
        let actual_mode = std::fs::metadata(&path)
            .expect("read file metadata after rejected chmod")
            .permissions()
            .mode()
            & 0o7777;
        assert_eq!(
            actual_mode & 0o6000,
            0,
            "rejected chmod must not have set SUID/SGID, got {actual_mode:04o}"
        );
    }
}

/// Backwards-compatible default: SUID/SGID modes are applied verbatim when the
/// toggle is off.
#[test]
fn suid_sgid_allowed_when_rejection_disabled() {
    use codedeploy_agent::installer::commands::ChangeModeCommand;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::NamedTempFile;

    let dangerous_modes: &[(&str, u32)] = &[
        ("4755", 0o4755), // SUID + rwxr-xr-x
        ("2755", 0o2755), // SGID + rwxr-xr-x
        ("6755", 0o6755), // SUID + SGID + rwxr-xr-x
    ];

    for (mode_str, expected) in dangerous_modes {
        let file = NamedTempFile::new().expect("temp file");
        let path = file.path().to_path_buf();

        let cmd = ChangeModeCommand::new(path.clone(), (*mode_str).to_string(), false, false);
        let mut cleanup: Vec<u8> = Vec::new();
        cmd.execute(&mut cleanup)
            .unwrap_or_else(|e| panic!("execute({mode_str}) must succeed when flag is off: {e}"));

        let actual_mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o7777;
        assert_eq!(
            actual_mode, *expected,
            "with rejection disabled, mode {mode_str} must be applied verbatim, got {actual_mode:04o}"
        );
    }
}

/// Post-install SUID via a lifecycle script — mitigated by privilege
/// separation.
///
/// **Security property:** A lifecycle script that runs `chmod u+s <binary>`
/// after deployment is constrained by the privilege separation architecture:
///
/// 1. Scripts execute in separate process groups via `setsid()`.
/// 2. Scripts run as the `runas` user specified in the AppSpec, not as root.
/// 3. A non-root user cannot set SUID on files they don't own.
/// 4. Even if SUID is set on an owned file, privilege escalation is limited
///    to that user's privileges — not root.
///
/// This is a defense-in-depth control; the recommended complementary control is
/// a post-deployment audit scanning for new SUID/SGID files. The process group
/// isolation itself is verified by the process-isolation tests.
///
/// This test is a documentation anchor — it verifies nothing at runtime but
/// serves as the canonical reference for why the scenario is considered
/// mitigated.
#[test]
fn privilege_separation_is_documented() {
    // Post-install SUID escalation is mitigated by privilege separation:
    //
    // Attack scenario:
    //   A lifecycle script (e.g. AfterInstall) runs `chmod u+s /deployment/binary`
    //   to create a set-user-ID binary that escalates privileges.
    //
    // Mitigation chain:
    //   1. Scripts execute in separate process groups via setsid()
    //      → Verified by the process-isolation tests
    //   2. Scripts run as the `runas` user, not root
    //      → Verified by the process-isolation tests
    //   3. Non-root users cannot set SUID on files they don't own
    //      → OS-level enforcement (EPERM from chmod syscall)
    //   4. Even if SUID is set on an owned file, escalation is limited
    //      to that user's privilege level — not root
    //
    // Residual risk:
    //   If the agent itself runs as root without configuring `runas`, the
    //   script inherits root privileges and CAN set SUID. This is an
    //   operational configuration concern, not a code defect.
    //
    // Recommended complementary control:
    //   Post-deployment audit that scans the deployment directory for files
    //   with (mode & 0o6000) != 0 and raises an alarm.
}

// ---------------------------------------------------------------------------
// SELinux Context Validation
// ---------------------------------------------------------------------------

/// `ChangeContextCommand` rejects unconfined types when the toggle is on.
#[test]
fn change_context_rejects_unconfined() {
    use codedeploy_agent::application_specification::AppSpec;
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeContextCommand;
    use std::fs;
    use tempfile::TempDir;

    let dangerous_types = ["unconfined_t", "kernel_t", "init_t"];
    let dir = TempDir::new().expect("temp dir");
    let target = dir.path().join("app");
    fs::write(&target, "test").expect("write target");

    for type_ in &dangerous_types {
        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: {type_}\n"
        );
        let spec = AppSpec::parse(&yaml)
            .expect("parse-time accepts unconfined types — rejection is at install time");
        let ctx = spec
            .permissions()
            .iter()
            .next()
            .expect("permission entry")
            .context()
            .expect("context")
            .clone();

        let cmd = ChangeContextCommand::new(target.clone(), ctx, true, false);
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        match result {
            Err(InstallerError::UnconfinedSelinuxRejected { type_: rejected }) => {
                assert_eq!(rejected, *type_);
            },
            other => panic!(
                "ChangeContextCommand should reject SELinux type '{type_}' \
                 with UnconfinedSelinuxRejected when reject_unconfined_selinux_in_bundle \
                 is enabled, got {other:?}"
            ),
        }
    }
}

/// Backwards-compatible default: unconfined types reach `semanage` when the
/// toggle is off.
/// Accepts `Ok` (semanage present and ran) or any non-`UnconfinedSelinuxRejected` error.
#[test]
fn unconfined_selinux_allowed_when_rejection_disabled() {
    use codedeploy_agent::application_specification::AppSpec;
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeContextCommand;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let target = dir.path().join("app");
    fs::write(&target, "test").expect("write target");

    for type_ in ["unconfined_t", "kernel_t", "init_t"] {
        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: {type_}\n"
        );
        let spec = AppSpec::parse(&yaml).expect("parse-time accepts unconfined types");
        let ctx = spec
            .permissions()
            .iter()
            .next()
            .expect("permission entry")
            .context()
            .expect("context")
            .clone();
        let cmd = ChangeContextCommand::new(target.clone(), ctx, false, false);
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        // The blocklist must NOT be enforced when the flag is disabled.
        // semanage/restorecon may not be installed in CI, so we accept either
        // Ok (binaries present and succeeded against a real SELinux system) or
        // an Io error (binaries missing or failed). The forbidden outcome is
        // UnconfinedSelinuxRejected — that would mean the default posture had
        // silently become restrictive.
        if let Err(InstallerError::UnconfinedSelinuxRejected { .. }) = result {
            panic!(
                "type {type_} must NOT be rejected when reject_unconfined_selinux_in_bundle is false"
            );
        }
    }
}

/// Valid SELinux contexts must be applied correctly.
///
/// **Security property:** Legitimate SELinux context types that are commonly
/// used for web applications, log files, and user data must be accepted
/// without error. The parser must correctly preserve the type string so that
/// downstream code (`ChangeContextCommand`) can pass it to `semanage`.
///
/// **Expected:** PASSES today — `AppSpec::parse()` accepts any syntactically
/// valid type string, and the accessor returns the exact value provided.
#[test]
fn change_context_accepts_valid_types() {
    use codedeploy_agent::application_specification::AppSpec;

    // Standard safe SELinux types used in real deployments
    let safe_types = [
        "httpd_sys_content_t",
        "var_log_t",
        "user_home_t",
        "bin_t",
        "etc_t",
        "tmp_t",
    ];

    for type_ in &safe_types {
        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: {type_}\n"
        );
        let spec = AppSpec::parse(&yaml)
            .unwrap_or_else(|e| panic!("AppSpec should accept safe SELinux type '{type_}': {e}"));

        // Verify the context type is correctly preserved
        let perm = spec
            .permissions()
            .iter()
            .next()
            .expect("Should have exactly one permission entry");
        let ctx = perm.context().expect("Permission should have a context");
        assert_eq!(
            ctx.type_(),
            *type_,
            "SELinux context type must be preserved exactly as specified"
        );
    }
}

/// Malformed SELinux contexts must not crash.
///
/// **Security property:** The AppSpec parser must handle malformed or
/// adversarial SELinux context specifications gracefully — returning a
/// well-defined error rather than panicking, producing undefined behavior,
/// or consuming unbounded resources. This includes missing required fields,
/// invalid MLS ranges, and extremely long type strings.
///
/// **Expected:** Partially passes today — `RawContext::validate()` requires
/// the `type` field and `MlsRange::parse()` validates range syntax. However,
/// a syntactically "valid" but semantically meaningless type string of
/// arbitrary length is accepted (length limiting is a deployment-time concern).
#[test]
fn malformed_selinux_context_handled_gracefully() {
    use codedeploy_agent::application_specification::AppSpec;

    // --- Missing required `type` field ---
    // The `type` field is mandatory for SELinux contexts. Omitting it must
    // produce a parse error, not a partial context with an empty type.
    let yaml_missing_type = "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      name: user_u\n";
    let result = AppSpec::parse(yaml_missing_type);
    assert!(result.is_err(), "Should reject SELinux context without required 'type' field");

    // --- Invalid MLS range ---
    // The `range` field must conform to the MLS range syntax (sN[-sN][:cN[.cN]]).
    // An invalid range string must produce a parse error.
    let yaml_invalid_range = "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: httpd_t\n      range: invalid\n";
    let result = AppSpec::parse(yaml_invalid_range);
    assert!(
        result.is_err(),
        "Should reject SELinux context with invalid MLS range 'invalid'"
    );

    // --- Extremely long type string ---
    // The parser must not panic or OOM when given a very long type string.
    // Whether this is accepted or rejected is implementation-dependent, but
    // it MUST NOT crash or cause undefined behavior.
    let long_type = "a".repeat(10_000);
    let yaml_long = format!(
        "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: {long_type}\n"
    );
    let result = AppSpec::parse(&yaml_long);
    // We assert it's either Ok or Err — the important thing is no panic/OOM.
    // If accepted, verify the type is preserved; if rejected, that's also fine.
    match result {
        Ok(spec) => {
            let ctx = spec
                .permissions()
                .iter()
                .next()
                .expect("permission entry")
                .context()
                .expect("context");
            assert_eq!(
                ctx.type_().len(),
                10_000,
                "If accepted, the long type string must be preserved in full"
            );
        },
        Err(_) => {
            // Rejection with an error is also acceptable — the key security
            // property is that the parser didn't panic or hang.
        },
    }
}

// ---------------------------------------------------------------------------
// Symlink-following TOCTOU on permission sinks (CWE-59/CWE-367)
// ---------------------------------------------------------------------------

/// A symlinked permission target must be rejected by ChangeOwnerCommand rather
/// than followed to the link target. Models the race where a local actor swaps
/// the copied file for `link -> /etc/passwd` before the chown runs.
#[test]
fn change_owner_rejects_symlink_target() {
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeOwnerCommand;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    // The "sensitive" file a local actor would try to re-own via the agent.
    let outside = dir.path().join("passwd_proxy");
    fs::write(&outside, "root:x:0:0").expect("write outside file");
    let link = dir.path().join("file1.txt");
    std::os::unix::fs::symlink(&outside, &link).expect("create symlink");

    let cmd = ChangeOwnerCommand::new(link.clone(), Some("root".to_string()), None, true);
    let mut cleanup: Vec<u8> = Vec::new();
    let result = cmd.execute(&mut cleanup);

    assert!(
        matches!(result, Err(InstallerError::SymlinkDestinationRejected { .. })),
        "chown on a symlinked destination must be rejected, got {result:?}"
    );
}

/// A symlinked permission target must be rejected by ChangeModeCommand rather
/// than followed (e.g. `chmod 0666` redirected onto /etc/shadow).
#[test]
fn change_mode_rejects_symlink_target() {
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeModeCommand;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let outside = dir.path().join("shadow_proxy");
    fs::write(&outside, "secret").expect("write outside file");
    let link = dir.path().join("file1.txt");
    std::os::unix::fs::symlink(&outside, &link).expect("create symlink");

    let cmd = ChangeModeCommand::new(link.clone(), "666".to_string(), false, true);
    let mut cleanup: Vec<u8> = Vec::new();
    let result = cmd.execute(&mut cleanup);

    assert!(
        matches!(result, Err(InstallerError::SymlinkDestinationRejected { .. })),
        "chmod on a symlinked destination must be rejected, got {result:?}"
    );

    // The link target must be untouched (rejected before any chmod ran).
    use std::os::unix::fs::PermissionsExt;
    let outside_mode = fs::metadata(&outside).expect("stat outside").permissions().mode() & 0o777;
    assert_ne!(outside_mode, 0o666, "link target mode must not have been changed");
}

/// A symlinked permission target must be rejected by ChangeContextCommand
/// before the `canonicalize`/`semanage` path (which resolves the link).
#[test]
fn change_context_rejects_symlink_target() {
    use codedeploy_agent::application_specification::AppSpec;
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeContextCommand;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let outside = dir.path().join("etc_proxy");
    fs::write(&outside, "data").expect("write outside file");
    let link = dir.path().join("file1.txt");
    std::os::unix::fs::symlink(&outside, &link).expect("create symlink");

    let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: httpd_sys_content_t\n";
    let spec = AppSpec::parse(yaml).expect("parse appspec");
    let ctx = spec.permissions().iter().next().unwrap().context().expect("context").clone();

    let cmd = ChangeContextCommand::new(link.clone(), ctx, false, true);
    let mut cleanup: Vec<u8> = Vec::new();
    let result = cmd.execute(&mut cleanup);

    assert!(
        matches!(result, Err(InstallerError::SymlinkDestinationRejected { .. })),
        "semanage relabel on a symlinked destination must be rejected, got {result:?}"
    );
}

/// A symlinked permission target must be rejected by ChangeAclCommand rather
/// than followed (setfacl redirected onto the link target).
#[test]
fn change_acl_rejects_symlink_target() {
    use codedeploy_agent::application_specification::AppSpec;
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeAclCommand;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let outside = dir.path().join("acl_proxy");
    fs::write(&outside, "data").expect("write outside file");
    let link = dir.path().join("file1.txt");
    std::os::unix::fs::symlink(&outside, &link).expect("create symlink");

    let yaml = "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    type:\n      - file\n    acls:\n      - \"u:root:rwx\"\n";
    let spec = AppSpec::parse(yaml).expect("parse appspec");
    let acl = spec.permissions().iter().next().unwrap().acls().expect("acls").clone();
    let cmd = ChangeAclCommand::new(link.clone(), acl, true);
    let mut cleanup: Vec<u8> = Vec::new();
    let result = cmd.execute(&mut cleanup);

    assert!(
        matches!(result, Err(InstallerError::SymlinkDestinationRejected { .. })),
        "setfacl on a symlinked destination must be rejected, got {result:?}"
    );
}

/// A regular-file permission target is still accepted (no regression for the
/// normal deployment path).
#[test]
fn change_owner_allows_regular_file() {
    use codedeploy_agent::installer::commands::ChangeOwnerCommand;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let file = dir.path().join("file1.txt");
    fs::write(&file, "data").expect("write file");

    // owner=None/group=None is a no-op chown that still exercises the sink and
    // the symlink guard; it must succeed on a regular file.
    let cmd = ChangeOwnerCommand::new(file.clone(), None, None, false);
    let mut cleanup: Vec<u8> = Vec::new();
    assert!(cmd.execute(&mut cleanup).is_ok(), "chown on a regular file must still succeed");
}

// ===========================================================================
// Property-Based Tests
// ===========================================================================

use codedeploy_agent::application_specification::Mode;
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Property 2: SUID/SGID bit invariant
// ---------------------------------------------------------------------------

// Any mode with SUID or SGID bits is rejected; the file's mode is unchanged.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_suid_sgid_bits_never_accepted(
        base_mode in 0o000u32..=0o777u32,
        suid_sgid_bits in prop_oneof![Just(0o4000u32), Just(0o2000u32), Just(0o6000u32)],
    ) {
        use codedeploy_agent::installer::InstallerError;
        use codedeploy_agent::installer::commands::ChangeModeCommand;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use tempfile::NamedTempFile;

        let mode_str = format!("{:o}", suid_sgid_bits | base_mode);
        let file = NamedTempFile::new().expect("temp file");
        let path = file.path().to_path_buf();
        let _ = Mode::from_octal(&mode_str);

        let cmd = ChangeModeCommand::new(path.clone(), mode_str.clone(), true, false);
        let mut cleanup: Vec<u8> = Vec::new();
        let result = cmd.execute(&mut cleanup);

        prop_assert!(
            matches!(result, Err(InstallerError::UnsafePermissionRejected { .. })),
            "must reject '{}', got {:?}", mode_str, result
        );

        let actual_mode = fs::metadata(&path).unwrap().permissions().mode() & 0o7777;
        prop_assert_eq!(actual_mode & 0o6000, 0, "got {:04o}", actual_mode);
    }
}

// ---------------------------------------------------------------------------
// Property 3: Unconfined SELinux context rejection
// ---------------------------------------------------------------------------

// Every blocklist member is rejected across randomised target paths.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_unconfined_selinux_always_rejected(
        type_idx in 0u8..3,
        suffix in "[a-z0-9_]{0,16}",
    ) {
        use codedeploy_agent::application_specification::AppSpec;
        use codedeploy_agent::installer::InstallerError;
        use codedeploy_agent::installer::commands::ChangeContextCommand;
        use std::fs;
        use tempfile::TempDir;

        let blocklist = ["unconfined_t", "kernel_t", "init_t"];
        let type_str = blocklist[type_idx as usize].to_string();

        let dir = TempDir::new().expect("temp dir");
        let target = dir.path().join(format!("app_{suffix}"));
        fs::write(&target, "test").expect("write target");

        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: {type_str}\n"
        );
        let spec = AppSpec::parse(&yaml).expect("parser accepts unconfined types");
        let ctx = spec.permissions().iter().next().unwrap().context().unwrap().clone();
        let cmd = ChangeContextCommand::new(target.clone(), ctx, true, false);
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        prop_assert!(
            matches!(result, Err(InstallerError::UnconfinedSelinuxRejected { .. })),
            "must reject '{}', got {:?}", type_str, result
        );
    }
}

// ---------------------------------------------------------------------------
// Property 9: Valid SELinux context application
// ---------------------------------------------------------------------------

// Property 9: Valid SELinux context application
// Validates: For any valid SELinux context type string matching the pattern
// [a-z][a-z0-9_]*_t (standard SELinux type naming), AppSpec::parse() accepts
// the context and preserves the type string exactly.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_valid_selinux_types_accepted(
        type_base in "[a-z][a-z0-9_]{1,20}",
    ) {
        use codedeploy_agent::application_specification::AppSpec;

        // Arrange: construct a valid SELinux type with standard _t suffix
        // Exclude dangerous types
        let type_str = format!("{type_base}_t");
        if type_str.contains("unconfined") || type_str.contains("kernel") || type_str.contains("init_t") {
            // Skip dangerous types — those are covered by the blocklist property above
            return Ok(());
        }

        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: {type_str}\n"
        );

        // Act
        let result = AppSpec::parse(&yaml);

        // Assert: valid types should be accepted and preserved
        let spec = result.unwrap_or_else(|_| panic!("AppSpec should accept safe SELinux type '{type_str}'"));
        let perm = spec.permissions().iter().next().expect("should have a permission entry");
        let ctx = perm.context().expect("permission should have a context");

        prop_assert_eq!(
            ctx.type_(), type_str.as_str(),
            "SELinux context type must be preserved exactly"
        );
    }
}

// ---------------------------------------------------------------------------
// Property 10: Malformed SELinux context handling
// ---------------------------------------------------------------------------

// Property 10: Malformed SELinux context handling
// Validates: For any random string used as a SELinux context type
// (including empty, control characters, extremely long), AppSpec::parse()
// either succeeds or returns Err — but never panics or crashes.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_malformed_selinux_context_no_crash(
        type_str in "\\PC{0,200}",
    ) {
        use codedeploy_agent::application_specification::AppSpec;

        // Arrange: use the random string as a SELinux context type
        // Must escape special YAML characters
        let escaped = type_str.replace('\\', "\\\\").replace('"', "\\\"");
        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: \"{escaped}\"\n"
        );

        // Act: must not panic — Ok or Err are both acceptable
        let _result = AppSpec::parse(&yaml);

        // Assert: no panic occurred (implicit — reaching this line proves it)
    }
}
