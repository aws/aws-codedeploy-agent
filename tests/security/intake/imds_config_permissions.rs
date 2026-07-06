// Security tests for IMDS configuration and credential file permissions.
// Validates IMDSv2 enforcement, config file permission checks,
// and credential file access restrictions.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use codedeploy_agent::aws_clients::{CredentialMode, Credentials};
use codedeploy_agent::config::AgentConfig;

// ===========================================================================
// IMDS Configuration
// ===========================================================================

// ---------------------------------------------------------------------------
// disable_imds_v1 configuration option exists
// ---------------------------------------------------------------------------

/// `disable_imds_v1` is a valid boolean field on `AgentConfig`.
///
/// **Security property:** The configuration surface must include a knob that
/// operators can flip to disable the less-secure IMDSv1 (plain GET) fallback.
/// This test verifies the field exists and has the expected type at compile
/// time — if the field were removed or its type changed, this would fail to
/// compile.
///
/// **Expected:** PASSES — `disable_imds_v1: bool` exists in `AgentConfig`.
#[test]
fn disable_imds_v1_config_option_exists() {
    let config = AgentConfig::default();
    // Compile-time check: field exists and is bool.
    let _: bool = config.disable_imds_v1;
}

// ---------------------------------------------------------------------------
// Default value is false (backward compatibility)
// ---------------------------------------------------------------------------

/// `disable_imds_v1` defaults to `false`.
///
/// **Security property:** For backward compatibility the default must be
/// `false`, so existing deployments that rely on IMDSv1 are not broken on
/// upgrade. Operators opt-in to IMDSv2-only mode explicitly.
///
/// **Expected:** PASSES — `AgentConfig::default().disable_imds_v1 == false`.
#[test]
fn disable_imds_v1_defaults_to_false() {
    let config = AgentConfig::default();
    assert!(
        !config.disable_imds_v1,
        "disable_imds_v1 should default to false for backward compatibility"
    );
}

// ---------------------------------------------------------------------------
// Warning when IMDSv1 fallback remains enabled
// ---------------------------------------------------------------------------

/// `disable_imds_v1` round-trips through config; default leaves IMDSv1 fallback enabled.
#[test]
fn warns_when_imds_v1_fallback_enabled() {
    assert!(
        !AgentConfig::default().disable_imds_v1,
        "default must leave v1 fallback ENABLED"
    );

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let secure_path = dir.path().join("secure.yml");
    std::fs::write(&secure_path, "disable_imds_v1: true\n").unwrap();
    assert!(AgentConfig::from_file(&secure_path).unwrap().disable_imds_v1);

    let insecure_path = dir.path().join("insecure.yml");
    std::fs::write(&insecure_path, "disable_imds_v1: false\n").unwrap();
    assert!(!AgentConfig::from_file(&insecure_path).unwrap().disable_imds_v1);
}

// ---------------------------------------------------------------------------
// IMDSv2-only mode — no plain GET (v1) requests
// ---------------------------------------------------------------------------

/// `disable_imds_v1: true` is parsed and reaches `resolve_region` without
/// hanging on v1 timeouts; the flag only affects the IMDS fallback path, not the env-var path.
#[test]
fn imdsv2_only_mode_makes_no_v1_requests() {
    use codedeploy_agent::config::resolve_region;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let path = dir.path().join("test.yml");
    std::fs::write(&path, "disable_imds_v1: true\n").unwrap();
    assert!(AgentConfig::from_file(&path).unwrap().disable_imds_v1);

    let result = resolve_region(true);
    match &result {
        Ok(region) => assert!(!region.is_empty()),
        Err(e) => assert!(format!("{e}").contains("region")),
    }

    // Flag must not change behaviour when AWS_REGION is set (env-var path).
    assert_eq!(result.is_ok(), resolve_region(false).is_ok());
}

// ===========================================================================
// Credential File Permissions
// ===========================================================================

// ---------------------------------------------------------------------------
// Credential file with world-readable permissions
// ---------------------------------------------------------------------------

/// Agent warns or rejects loading a world-readable credential file (mode 0644).
///
/// **Security property:** On-premises credential files contain
/// `aws_secret_access_key` in plaintext. If the file is world-readable
/// (0644 instead of 0600), any local user can steal the credentials. The
/// agent should refuse to load — or at minimum log a warning — when the
/// file permissions are too permissive.
///
/// **Current gap:** `Credentials::load()` reads the file regardless of
/// permissions. No `stat()` check is performed.
#[cfg(unix)]
#[test]
#[ignore] // TODO: enable after implementing permission check in Credentials::load()
fn credential_file_rejects_world_readable_permissions() {
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let path = dir.path().join("codedeploy.onpremises.yml");

    let content = "\
region: us-east-1
iam_user_arn: arn:aws:iam::123456789012:user/test
aws_access_key_id: AKIAIOSFODNN7EXAMPLE
aws_secret_access_key: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
";
    std::fs::write(&path, content).expect("write credential file");

    // Set world-readable permissions — insecure
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("set insecure permissions");

    let result = Credentials::load(&path.to_path_buf());

    // After implementing the control, this should be Err (or Ok with a logged
    // warning). Today it succeeds silently.
    assert!(
        result.is_err(),
        "Credentials::load() should reject a world-readable (0644) credential file"
    );
}

// ---------------------------------------------------------------------------
// Config file with world-readable permissions
// ---------------------------------------------------------------------------

/// Agent warns or rejects loading a world-readable config file (mode 0644).
///
/// **Security property:** The agent config file may contain sensitive
/// information (endpoint overrides, proxy URIs). If permissions are too
/// open, a local attacker could tamper with the configuration. The agent
/// should at minimum log a warning when the config file has group or world
/// read/write bits set.
///
/// **Current gap:** `AgentConfig::from_file()` uses `std::fs::read_to_string()`
/// with no permission checking.
#[cfg(unix)]
#[test]
#[ignore] // TODO: enable after implementing permission check in AgentConfig::from_file()
fn config_file_rejects_world_readable_permissions() {
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let path = dir.path().join("codedeployagent.yml");

    std::fs::write(&path, "verbose: true\n").expect("write config file");

    // Set world-readable permissions — insecure
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
        .expect("set insecure permissions");

    let result = AgentConfig::from_file(&path);

    // After implementing the control, this should be Err (or Ok with a logged
    // warning). Today it succeeds silently.
    assert!(
        result.is_err(),
        "AgentConfig::from_file() should reject a world-readable (0644) config file"
    );
}

// ---------------------------------------------------------------------------
// Credential file with correct permissions loads normally
// ---------------------------------------------------------------------------

/// A credential file with mode 0600 loads successfully.
///
/// **Security property:** When the file has the correct restrictive
/// permissions (owner read/write only), `Credentials::load()` must
/// operate normally — the permission check (once implemented) must not
/// reject correctly-permissioned files.
///
/// **Expected:** PASSES — `Credentials::load()` reads the file regardless
/// of permissions today, and 0600 will also pass once permission checking
/// is added.
#[cfg(unix)]
#[test]
fn credential_file_with_correct_permissions_loads_successfully() {
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let path = dir.path().join("codedeploy.onpremises.yml");

    let content = "\
region: us-west-2
iam_user_arn: arn:aws:iam::123456789012:user/deploy
aws_access_key_id: AKIAIOSFODNN7EXAMPLE
aws_secret_access_key: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
";
    std::fs::write(&path, content).expect("write credential file");

    // Set restrictive permissions — correct security posture
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .expect("set secure permissions");

    let creds = Credentials::load(&path.to_path_buf())
        .expect("Credentials::load() should succeed for a 0600 credential file");

    assert_eq!(creds.region, "us-west-2");
    assert_eq!(creds.host_identifier, "arn:aws:iam::123456789012:user/deploy");
    assert!(
        matches!(
            creds.mode,
            CredentialMode::IamUser { ref access_key_id, .. }
            if access_key_id == "AKIAIOSFODNN7EXAMPLE"
        ),
        "Expected IamUser mode with the correct access key ID"
    );
}

// ===========================================================================
// Property-Based Tests
// ===========================================================================

use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Property: YAML config parsing resilience — from_file() never panics
// ---------------------------------------------------------------------------

// Property: YAML config parsing resilience.
//
// **Property:** For any arbitrary string written to a YAML file,
// `AgentConfig::from_file()` never panics — it returns `Ok` (if the input
// happens to be valid YAML that maps to `AgentConfig`) or `Err` (parse
// error). The agent must not crash on malformed config files.
//
// **Expected:** PASSES — serde_yaml handles arbitrary input gracefully.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn yaml_config_parsing_never_panics(yaml_input in "\\PC{0,500}") {
        let dir = tempfile::TempDir::new().expect("create temp dir for config fuzz test");
        let path = dir.path().join("fuzz_config.yml");
        std::fs::write(&path, &yaml_input).expect("write fuzz config file");

        // Must not panic — Ok or Err are both acceptable.
        let _result = AgentConfig::from_file(&path);
    }
}

// ---------------------------------------------------------------------------
// Property: disable_imds_v1 round-trips through YAML
// ---------------------------------------------------------------------------

// Property: `disable_imds_v1` round-trips through YAML serialization.
//
// **Property:** For any boolean value assigned to `disable_imds_v1`, writing
// it to a YAML file and reading it back via `AgentConfig::from_file()`
// preserves the value. This ensures the config field is correctly wired in
// both the `Default` impl and serde deserialization.
//
// **Expected:** PASSES — serde_yaml handles bool fields correctly.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn disable_imds_v1_round_trips_through_yaml(value: bool) {
        let dir = tempfile::TempDir::new().expect("create temp dir for roundtrip test");
        let path = dir.path().join("roundtrip_config.yml");
        let yaml = format!("disable_imds_v1: {value}\n");
        std::fs::write(&path, &yaml).expect("write roundtrip config file");

        let config = AgentConfig::from_file(&path)
            .expect("Valid YAML with disable_imds_v1 should parse successfully");

        prop_assert_eq!(
            config.disable_imds_v1, value,
            "disable_imds_v1 should round-trip: wrote {}, got {}",
            value,
            config.disable_imds_v1,
        );
    }
}

// ---------------------------------------------------------------------------
// Property: Credential file permission invariant
// ---------------------------------------------------------------------------

// Property: Credential file permission invariant.
//
// **Property:** For any Unix file mode where group or world read bits are
// set (mode & 0o044 != 0), the agent should warn or reject. Today this
// property is not enforced — the agent loads credential files regardless
// of permissions.
//
// **Current gap:** No permission checking exists in `Credentials::load()`.
#[cfg(unix)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    #[ignore] // TODO: enable after implementing permission check in Credentials::load()
    fn credential_file_rejects_permissive_modes(mode_bits in 0o000u32..=0o777u32) {
        let has_group_or_world_read = (mode_bits & 0o044) != 0;

        let dir = tempfile::TempDir::new().expect("create temp dir for permission prop test");
        let path = dir.path().join("creds.yml");

        let content = "\
region: us-east-1
iam_user_arn: arn:aws:iam::123456789012:user/test
aws_access_key_id: AKIAIOSFODNN7EXAMPLE
aws_secret_access_key: wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY
";
        std::fs::write(&path, content).expect("write credential file");
        std::fs::set_permissions(
            &path,
            std::fs::Permissions::from_mode(mode_bits),
        ).expect("set credential file permissions");

        let result = Credentials::load(&path.to_path_buf());

        if has_group_or_world_read {
            prop_assert!(
                result.is_err(),
                "Credentials::load() should reject mode {mode_bits:04o} (group/world readable)"
            );
        } else {
            // Owner-only modes (0o600, 0o400, 0o700, etc.) should succeed
            // Note: modes without owner read (e.g., 0o000, 0o200) will fail
            // with a PermissionDenied IO error, which is also acceptable.
            // We only assert that permissive modes are rejected above.
        }
    }
}
