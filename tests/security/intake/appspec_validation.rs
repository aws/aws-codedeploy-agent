//! Security tests for AppSpec parsing and validation (TO-007).
//!
//! These tests verify that the AppSpec parser correctly rejects or sanitizes
//! dangerous inputs such as SUID/SGID permission modes, unconfined SELinux
//! contexts, and malformed YAML.
//!
//! Tests marked `#[ignore]` document known security gaps — they will pass once
//! the corresponding production-code controls are implemented.

use aws_codedeploy_agent::application_specification::{AppSpec, Mode};

// ---------------------------------------------------------------------------
// TC-007-01: AppSpec with SUID Permissions — Rejection or Stripping
// ---------------------------------------------------------------------------

/// TO-007 / TC-007-01: SUID/SGID modes must be rejected at parse time.
///
/// **Security property:** An attacker-controlled AppSpec must not be able to set
/// the SUID, SGID, or combined SUID+SGID bits on deployed files.  If the
/// AppSpec specifies `mode: "4755"`, `"6755"`, `"2755"`, or `"7777"`, the
/// parser should return an error rather than silently accepting a privileged
/// mode.
///
/// **Current gap:** `Mode::from_octal()` accepts any valid 4-digit octal
/// string including those with the SUID (0o4000) and SGID (0o2000) bits set.
/// No validation rejects these dangerous modes during AppSpec parsing.
#[test]
#[ignore] // TODO: enable after implementing SUID/SGID rejection in Mode::from_octal() or RawPermission::validate()
fn appspec_rejects_suid_permissions() {
    let suid_modes = ["4755", "6755", "2755", "7777"];
    for mode_str in &suid_modes {
        let yaml = format!(
            "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    mode: \"{mode_str}\"\n"
        );
        let result = AppSpec::parse(&yaml);
        assert!(
            result.is_err(),
            "AppSpec should reject SUID/SGID mode {mode_str} but accepted it"
        );
    }
}

/// TO-007 / TC-007-01 alt: SUID/SGID bits are detectable in parsed modes.
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
// TC-007-02: AppSpec with Dangerous SELinux Context — Rejection
// ---------------------------------------------------------------------------

/// TO-007 / TC-007-02: Dangerous SELinux contexts must be rejected.
///
/// **Security property:** An AppSpec must not be allowed to set `unconfined_t`
/// or similar dangerous SELinux context types on deployed files.  The
/// `unconfined_t` type effectively disables SELinux enforcement for the
/// labelled process/file, which an attacker could exploit to escape mandatory
/// access controls.
///
/// **Current gap:** `RawContext::validate()` accepts any string value for the
/// `type` field, including `unconfined_t`.  There is no allowlist or blocklist
/// of acceptable SELinux types.
#[test]
#[ignore] // TODO: enable after implementing SELinux context allowlist/blocklist validation in RawContext::validate()
fn appspec_rejects_unconfined_selinux_context() {
    // "unconfined_t" as the SELinux type field
    let yaml_unconfined_type = "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      type: unconfined_t\n";
    let result = AppSpec::parse(yaml_unconfined_type);
    assert!(
        result.is_err(),
        "AppSpec should reject SELinux context type 'unconfined_t' but accepted it"
    );

    // Full unconfined context: user + type both unconfined
    let yaml_full_unconfined = "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      name: unconfined_u\n      type: unconfined_t\n      range: s0\n";
    let result = AppSpec::parse(yaml_full_unconfined);
    assert!(
        result.is_err(),
        "AppSpec should reject full unconfined SELinux context but accepted it"
    );

    // system_u with unconfined_t — the type alone is dangerous
    let yaml_system_unconfined = "version: 0.0\nos: linux\npermissions:\n  - object: /app\n    context:\n      name: system_u\n      type: unconfined_t\n      range: s0\n";
    let result = AppSpec::parse(yaml_system_unconfined);
    assert!(
        result.is_err(),
        "AppSpec should reject unconfined_t even with system_u user but accepted it"
    );
}

// ---------------------------------------------------------------------------
// TC-007-03: Malformed AppSpec — Schema Validation
// ---------------------------------------------------------------------------

/// TO-007 / TC-007-03: Malformed AppSpec must produce clear parse errors.
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

/// TO-007 / TC-007-03 supplement: Error variants carry actionable information.
///
/// **Security property:** Parse errors must be specific enough for operators to
/// diagnose and fix AppSpec issues without leaking internal implementation
/// details. Error messages should identify which field or section is invalid.
#[test]
fn appspec_error_messages_are_actionable() {
    use aws_codedeploy_agent::application_specification::ParseError;

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
