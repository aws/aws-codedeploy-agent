//! Security tests for installer permission handling (CR 1c: TO-011, TO-012).
//!
//! **TO-011 — SUID/SGID Bit Handling:**
//! Validates that the deployment pipeline strips or blocks SUID (0o4000) and
//! SGID (0o2000) bits. Archives containing set-user-ID or set-group-ID
//! binaries, and AppSpec files requesting those modes via `chmod`, must not
//! result in deployed files carrying those dangerous privilege-escalation bits.
//!
//! **TO-012 — SELinux Context Validation:**
//! Validates that the SELinux context handling rejects dangerous context types
//! (`unconfined_t`, `kernel_t`, `init_t`) that would effectively disable
//! mandatory access controls, accepts legitimate types, and handles malformed
//! context specifications gracefully without panics or undefined behavior.
//!
//! Tests marked `#[ignore]` document known security gaps — they will pass once
//! the corresponding production-code controls are implemented.

// ---------------------------------------------------------------------------
// TO-011: SUID/SGID Bit Handling
// ---------------------------------------------------------------------------

/// TO-011 / TC-011-01: SUID/SGID bits must be stripped from extracted files.
///
/// **Security property:** When an archive containing files with the SUID
/// (0o4000) or SGID (0o2000) bits set is extracted by the bundle unpacker,
/// the resulting files on disk must have those bits cleared. A malicious
/// actor who controls the archive content must not be able to deploy
/// set-user-ID binaries that escalate privileges.
///
/// **Current gap:** `bundle_unpacker::unpack()` delegates to system `tar`
/// without post-extraction stripping of SUID/SGID bits.
#[test]
#[ignore] // TODO: enable after implementing post-extraction SUID stripping in bundle_unpacker::unpack()
fn extracted_files_have_no_suid_sgid() {
    use aws_codedeploy_agent::host_command::bundle_unpacker;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use tempfile::TempDir;

    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();

    // Create files with SUID, SGID, and combined SUID+SGID bits
    let test_cases: &[(&str, u32)] = &[
        ("suid_binary", 0o4755), // SUID only
        ("sgid_binary", 0o2755), // SGID only
        ("both_binary", 0o6755), // SUID + SGID
    ];

    for (name, mode) in test_cases {
        let binary = src.join(name);
        std::fs::write(&binary, "#!/bin/sh\necho pwned").unwrap();
        let mut perms = std::fs::metadata(&binary).unwrap().permissions();
        perms.set_mode(*mode);
        std::fs::set_permissions(&binary, perms).unwrap();
    }

    // Create a tar archive containing all three files
    let tar_path = dir.path().join("suid.tar");
    let output = Command::new("tar")
        .args([
            "-cf",
            tar_path.to_str().unwrap(),
            "-C",
            src.to_str().unwrap(),
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
    bundle_unpacker::unpack(&tar_path, &dest, "tar").expect("unpack() should succeed");

    // Verify SUID/SGID bits are stripped from every extracted file
    for (name, original_mode) in test_cases {
        let extracted = dest.join(name);
        assert!(extracted.exists(), "Expected extracted file: {name}");
        let mode = std::fs::metadata(&extracted).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o6000,
            0,
            "SUID/SGID bits must be stripped after extraction: \
             file={name}, original={original_mode:04o}, extracted={mode:04o}"
        );
    }
}

/// TO-011 / TC-011-02: ChangeModeCommand must strip SUID/SGID before applying.
///
/// **Security property:** When an AppSpec specifies a permission mode with the
/// SUID (0o4000) or SGID (0o2000) bits set (e.g. `mode: "4755"`), the
/// `ChangeModeCommand` must either reject the mode or silently strip the
/// dangerous bits before calling `fs::set_permissions()`. This prevents
/// an attacker who controls the AppSpec from deploying privilege-escalation
/// binaries via the permissions section.
///
/// **Current gap:** `ChangeModeCommand::execute()` applies the mode string
/// as-is via `u32::from_str_radix(&self.mode, 8)` — no SUID/SGID masking.
#[test]
#[ignore] // TODO: enable after implementing SUID/SGID stripping in ChangeModeCommand::execute()
fn change_mode_strips_suid_sgid() {
    use aws_codedeploy_agent::installer::commands::ChangeModeCommand;
    use std::os::unix::fs::PermissionsExt;
    use tempfile::NamedTempFile;

    let file = NamedTempFile::new().expect("failed to create temp file");
    let path = file.path().to_path_buf();

    // Modes with SUID, SGID, and combined bits
    let dangerous_modes: &[(&str, u32)] = &[
        ("4755", 0o755),  // SUID + rwxr-xr-x → should become 0755
        ("2755", 0o755),  // SGID + rwxr-xr-x → should become 0755
        ("6755", 0o755),  // SUID+SGID + rwxr-xr-x → should become 0755
        ("7777", 0o1777), // SUID+SGID+sticky + rwxrwxrwx → sticky is OK, SUID/SGID stripped
    ];

    for (mode_str, expected_base) in dangerous_modes {
        let cmd = ChangeModeCommand::new(path.clone(), mode_str.to_string());
        let mut cleanup: Vec<u8> = Vec::new();
        cmd.execute(&mut cleanup)
            .unwrap_or_else(|e| panic!("execute() failed for mode {mode_str}: {e}"));

        let actual_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o7777;

        assert_eq!(
            actual_mode & 0o6000,
            0,
            "ChangeModeCommand must strip SUID/SGID bits: \
             requested={mode_str}, got={actual_mode:04o}"
        );
        assert_eq!(
            actual_mode, *expected_base,
            "Base permissions should be {expected_base:04o} after stripping SUID/SGID \
             from {mode_str}, got {actual_mode:04o}"
        );
    }
}

/// TO-011 / TC-011-03: Post-install SUID via lifecycle script — mitigated by
/// privilege separation.
///
/// **Security property:** A lifecycle script that runs `chmod u+s <binary>`
/// after deployment is constrained by the privilege separation architecture:
///
/// 1. Scripts execute in separate process groups via `setsid()` (Phase 3 / TO-005).
/// 2. Scripts run as the `runas` user specified in the AppSpec, not as root.
/// 3. A non-root user cannot set SUID on files they don't own.
/// 4. Even if SUID is set on an owned file, privilege escalation is limited
///    to that user's privileges — not root.
///
/// **Mitigation status:** MITIGATED by privilege separation. This is a
/// defense-in-depth control; the recommended complementary control is a
/// post-deployment audit scanning for new SUID/SGID files. The process group
/// isolation is verified in Phase 3 (TO-005 / TC-005-01 and TC-005-02).
///
/// This test is a documentation anchor — it verifies nothing at runtime but
/// serves as the canonical reference for why TC-011-03 is considered mitigated.
#[test]
fn privilege_separation_is_documented() {
    // TC-011-03 is MITIGATED by privilege separation:
    //
    // Attack scenario:
    //   A lifecycle script (e.g. AfterInstall) runs `chmod u+s /deployment/binary`
    //   to create a set-user-ID binary that escalates privileges.
    //
    // Mitigation chain:
    //   1. Scripts execute in separate process groups via setsid()
    //      → Verified by Phase 3 / TO-005 / TC-005-01
    //   2. Scripts run as the `runas` user, not root
    //      → Verified by Phase 3 / TO-005 / TC-005-02
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
// TO-012: SELinux Context Validation
// ---------------------------------------------------------------------------

/// TO-012 / TC-012-01: Unconfined SELinux contexts must be rejected.
///
/// **Security property:** The SELinux context type field must not accept
/// dangerous types that effectively disable mandatory access controls.
/// Types like `unconfined_t`, `kernel_t`, and `init_t` would allow a
/// deployed application to bypass SELinux enforcement, defeating the
/// purpose of MAC-based confinement.
///
/// **Current gap:** `ChangeContextCommand::execute()` passes any type string
/// to `semanage fcontext` without validation. There is no allowlist or
/// blocklist of dangerous SELinux types.
///
/// **Note:** Since `ChangeContextCommand::new_with_ops()` and `MockSeLinuxOps`
/// are only available within `src/` unit tests (`#[cfg(test)]`), this
/// integration test validates the rejection at the AppSpec parsing layer
/// instead. If context-type validation is implemented in the
/// `ChangeContextCommand` itself, a corresponding unit test should be added
/// in `src/installer/commands/change_context_command.rs`.
#[test]
#[ignore] // TODO: enable after implementing SELinux context type blocklist/allowlist validation
fn change_context_rejects_unconfined() {
    use aws_codedeploy_agent::application_specification::AppSpec;

    let dangerous_types = ["unconfined_t", "kernel_t", "init_t"];

    for type_ in &dangerous_types {
        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: {type_}\n"
        );
        let result = AppSpec::parse(&yaml);

        // The dangerous type should be rejected either at parse time
        // (returning an error from AppSpec::parse) or by the installer
        // command at apply time. This test checks the parse-time gate.
        assert!(
            result.is_err(),
            "Should reject dangerous SELinux context type '{type_}' \
             but AppSpec::parse() accepted it. The type must be blocked \
             either in RawContext::validate() or ChangeContextCommand::execute()."
        );
    }
}

/// TO-012 / TC-012-02: Valid SELinux contexts must be applied correctly.
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
    use aws_codedeploy_agent::application_specification::AppSpec;

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

/// TO-012 / TC-012-03: Malformed SELinux contexts must not crash.
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
    use aws_codedeploy_agent::application_specification::AppSpec;

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
