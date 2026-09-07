//! Security tests for AppSpec parsing and validation.
//!
//! These tests verify that the AppSpec parser correctly rejects or sanitizes
//! dangerous inputs such as SUID/SGID permission modes, unconfined SELinux
//! contexts, and malformed YAML.
//!
//! Tests marked `#[ignore]` document known security gaps — they will pass once
//! the corresponding production-code controls are implemented.

use codedeploy_agent::application_specification::{AppSpec, Mode};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// AppSpec with SUID Permissions — Rejection or Stripping
// ---------------------------------------------------------------------------

/// Parser accepts SUID/SGID modes; install-time rejection happens in `ChangeModeCommand`.
#[cfg(unix)]
#[test]
fn appspec_rejects_suid_permissions() {
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeModeCommand;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let target = dir.path().join("app");
    fs::write(&target, "test").expect("write target");

    let suid_modes = ["4755", "6755", "2755", "7777"];
    for mode_str in &suid_modes {
        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    mode: \"{mode_str}\"\n"
        );
        AppSpec::parse(&yaml).expect("parser accepts SUID modes");

        let cmd = ChangeModeCommand::new(target.clone(), (*mode_str).to_string(), true, false);
        let mut cleanup: Vec<u8> = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(
            matches!(result, Err(InstallerError::UnsafePermissionRejected { .. })),
            "must reject {mode_str}, got {result:?}"
        );
    }
}

/// SUID/SGID bits are detectable in parsed modes.
///
/// **Security property:** The `Mode` type correctly models the SUID, SGID, and
/// sticky bits so that downstream code (installer, auditing) can detect them.
/// This test confirms the detection API works today — the gap is that nothing
/// **acts** on this detection during validation.
///
/// **Expected:** PASSES today — `Mode::from_octal()` correctly parses special
/// bits and `setuid()` / `setgid()` accessors return the right values.
#[test]
fn mode_suid_bits_are_detectable() {
    // Mode 4755: SUID + rwxr-xr-x
    let mode = Mode::from_octal("4755").unwrap();
    assert!(mode.setuid(), "Mode 4755 should have SUID bit set");
    assert!(!mode.setgid(), "Mode 4755 should NOT have SGID bit set");
    assert!(!mode.sticky(), "Mode 4755 should NOT have sticky bit set");

    // Mode 6755: SUID + SGID + rwxr-xr-x
    let mode = Mode::from_octal("6755").unwrap();
    assert!(mode.setuid(), "Mode 6755 should have SUID bit set");
    assert!(mode.setgid(), "Mode 6755 should have SGID bit set");
    assert!(!mode.sticky(), "Mode 6755 should NOT have sticky bit set");

    // Mode 2755: SGID + rwxr-xr-x
    let mode = Mode::from_octal("2755").unwrap();
    assert!(!mode.setuid(), "Mode 2755 should NOT have SUID bit set");
    assert!(mode.setgid(), "Mode 2755 should have SGID bit set");
    assert!(!mode.sticky(), "Mode 2755 should NOT have sticky bit set");

    // Mode 7777: SUID + SGID + sticky + rwxrwxrwx
    let mode = Mode::from_octal("7777").unwrap();
    assert!(mode.setuid(), "Mode 7777 should have SUID bit set");
    assert!(mode.setgid(), "Mode 7777 should have SGID bit set");
    assert!(mode.sticky(), "Mode 7777 should have sticky bit set");
}

// ---------------------------------------------------------------------------
// AppSpec with Dangerous SELinux Context — Rejection
// ---------------------------------------------------------------------------

/// Parser accepts unconfined types; install-time rejection happens in `ChangeContextCommand`.
#[cfg(unix)]
#[test]
fn appspec_rejects_unconfined_selinux_context() {
    use codedeploy_agent::installer::InstallerError;
    use codedeploy_agent::installer::commands::ChangeContextCommand;
    use std::fs;
    use tempfile::TempDir;

    let dir = TempDir::new().expect("temp dir");
    let target = dir.path().join("app");
    fs::write(&target, "test").expect("write target");

    let yamls = [
        "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: unconfined_t\n",
        "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      name: unconfined_u\n      type: unconfined_t\n      range: s0\n",
    ];
    for yaml in &yamls {
        let spec = AppSpec::parse(yaml).expect("parser accepts unconfined types");
        let ctx = spec.permissions().iter().next().unwrap().context().unwrap().clone();
        let cmd = ChangeContextCommand::new(target.clone(), ctx, true, false);
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(
            matches!(result, Err(InstallerError::UnconfinedSelinuxRejected { .. })),
            "must reject unconfined context, got {result:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Malformed AppSpec — Schema Validation
// ---------------------------------------------------------------------------

/// Malformed AppSpec must produce clear parse errors.
///
/// **Security property:** The AppSpec parser must reject structurally invalid
/// input with well-defined error variants rather than panicking, silently
/// ignoring missing fields, or producing partially-initialized structs that
/// could lead to undefined deployment behavior.
///
/// **Expected:** PASSES today — `RawAppSpec::validate()` already handles all
/// of these malformed-input cases.
#[test]
fn appspec_rejects_malformed_input() {
    // --- Missing required fields ---

    // Missing version field → the YAML deserializer requires `version` in RawAppSpec
    let result = AppSpec::parse("os: linux\n");
    assert!(result.is_err(), "Should reject AppSpec missing 'version' field");

    // Missing OS field → the YAML deserializer requires `os` in RawAppSpec
    let result = AppSpec::parse("version: 0.0\n");
    assert!(result.is_err(), "Should reject AppSpec missing 'os' field");

    // --- Hook validation ---

    // Hook with missing location (required field for a script entry)
    let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - timeout: 30\n";
    let result = AppSpec::parse(yaml);
    assert!(result.is_err(), "Should reject hook entry with missing 'location'");

    // Hook with empty location string
    let yaml = "version: 0.0\nos: linux\nhooks:\n  ApplicationStart:\n    - location: \"\"\n";
    let result = AppSpec::parse(yaml);
    assert!(result.is_err(), "Should reject hook entry with empty location string");

    // --- Files section validation ---

    // Missing source in file mapping
    let yaml = "version: 0.0\nos: linux\nfiles:\n  - destination: /dest\n";
    let result = AppSpec::parse(yaml);
    assert!(result.is_err(), "Should reject file mapping without 'source'");

    // Missing destination in file mapping
    let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /src\n";
    let result = AppSpec::parse(yaml);
    assert!(result.is_err(), "Should reject file mapping without 'destination'");

    // --- Invalid YAML syntax ---

    let result = AppSpec::parse("{{{invalid");
    assert!(result.is_err(), "Should reject invalid YAML syntax");

    // --- Empty file ---

    let result = AppSpec::parse("");
    assert!(result.is_err(), "Should reject empty input");

    let result = AppSpec::parse("   \n  \n  ");
    assert!(result.is_err(), "Should reject whitespace-only input");

    // --- Permissions on Windows (cross-validation) ---

    let yaml = "version: 0.0\nos: windows\npermissions:\n  - object: /tmp\n    mode: \"0755\"\n";
    let result = AppSpec::parse(yaml);
    assert!(result.is_err(), "Should reject permissions section when os=windows");
}

/// Error variants carry actionable information.
///
/// **Security property:** Parse errors must be specific enough for operators to
/// diagnose and fix AppSpec issues without leaking internal implementation
/// details. Error messages should identify which field or section is invalid.
#[test]
fn appspec_error_messages_are_actionable() {
    use codedeploy_agent::application_specification::ParseError;

    // Invalid version → error should mention the bad value
    let result = AppSpec::parse("version: 1.0\nos: linux\n");
    match result {
        Err(ParseError::InvalidVersion(v)) => {
            assert!(v.contains("1"), "InvalidVersion should carry the offending value, got: {v}");
        },
        other => panic!("Expected InvalidVersion, got: {other:?}"),
    }

    // Unsupported OS → error should mention the bad OS string
    let result = AppSpec::parse("version: 0.0\nos: freebsd\n");
    match result {
        Err(ParseError::UnsupportedOs(os)) => {
            assert_eq!(os, "freebsd", "UnsupportedOs should carry the offending value");
        },
        other => panic!("Expected UnsupportedOs, got: {other:?}"),
    }

    // Empty file → specific error variant
    let result = AppSpec::parse("");
    assert!(
        matches!(result, Err(ParseError::EmptyFile)),
        "Empty input should produce EmptyFile variant, got: {result:?}"
    );

    // Permissions on Windows → specific error variant
    let yaml = "version: 0.0\nos: windows\npermissions:\n  - object: /tmp\n    mode: \"0755\"\n";
    let result = AppSpec::parse(yaml);
    assert!(
        matches!(result, Err(ParseError::PermissionsOnWindows)),
        "Windows + permissions should produce PermissionsOnWindows variant, got: {result:?}"
    );

    // Missing source → specific error variant
    let yaml = "version: 0.0\nos: linux\nfiles:\n  - destination: /dest\n";
    let result = AppSpec::parse(yaml);
    assert!(
        matches!(result, Err(ParseError::MissingSource)),
        "File without source should produce MissingSource variant, got: {result:?}"
    );

    // Missing destination → specific error variant, includes source path
    let yaml = "version: 0.0\nos: linux\nfiles:\n  - source: /src\n";
    let result = AppSpec::parse(yaml);
    match result {
        Err(ParseError::MissingDestination(src)) => {
            assert_eq!(src, "/src", "MissingDestination should carry the source path");
        },
        other => panic!("Expected MissingDestination, got: {other:?}"),
    }
}

// ===========================================================================
// Property-Based Tests
// ===========================================================================

// ---------------------------------------------------------------------------
// Property 4: Malformed AppSpec rejection
// ---------------------------------------------------------------------------

// Property 4: Malformed AppSpec rejection
// Validates: For any malformed AppSpec YAML (missing version, missing OS,
// invalid YAML syntax, random garbage), AppSpec::parse() returns Err and
// never panics.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_malformed_appspec_never_panics(input in "\\PC{0,500}") {
        // Arrange: arbitrary string that is overwhelmingly unlikely to be valid AppSpec
        // Act: attempt to parse
        let _result = AppSpec::parse(&input);
        // Assert: no panic occurred — Ok or Err are both acceptable
    }
}

// Property 4: Malformed AppSpec rejection (structured variants)
// Validates: For any AppSpec YAML with specific structural defects (missing
// required fields, wrong types), AppSpec::parse() returns Err.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_malformed_appspec_missing_fields_rejected(
        random_field in "[a-z_]{1,20}",
        random_value in "[a-zA-Z0-9 ]{0,50}",
    ) {
        // Arrange: YAML with a random field but missing the required `version` and `os`
        let yaml = format!("{random_field}: {random_value}\n");

        // Act
        let result = AppSpec::parse(&yaml);

        // Assert: must be rejected (missing version and/or os)
        prop_assert!(
            result.is_err(),
            "AppSpec with only '{random_field}' should be rejected, got Ok"
        );
    }
}
