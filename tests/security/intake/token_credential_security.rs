// Security tests for credential memory protection and token security.
// Validates credential Debug redaction, token generation,
// file permissions, and invalid token rejection.

use std::fs;
use tempfile::TempDir;

use codedeploy_agent::aws_clients::credentials::{CredentialMode, Credentials};
use codedeploy_agent::command_port::auth::Auth;

// ===========================================================================
// Credential Memory Protection
// ===========================================================================

// ---------------------------------------------------------------------------
// No Unsafe Code in Credential Paths
// ---------------------------------------------------------------------------

/// Validates: No unsafe code blocks exist in credential handling paths, ensuring
/// Rust's memory safety guarantees are not bypassed for sensitive credential data.
///
/// **Expected:** PASSES — `credentials.rs` contains no `unsafe` blocks.
/// Note: The crate-level `#![forbid(unsafe_code)]` already prevents this at
/// compile time, but this test serves as a defense-in-depth static analysis check
/// that runs even if the forbid attribute is accidentally removed.
#[test]
fn no_unsafe_in_credential_code() {
    let source = fs::read_to_string("src/aws_clients/credentials.rs")
        .expect("Failed to read credentials.rs — run tests from project root");

    // Count occurrences of "unsafe" that aren't inside comments or strings.
    // A simple text match is sufficient because the crate also has #![forbid(unsafe_code)].
    let unsafe_count = source.matches("unsafe").count();
    assert_eq!(
        unsafe_count, 0,
        "Found {unsafe_count} 'unsafe' occurrences in credential code — \
         credential handling must use only safe Rust"
    );
}

// ---------------------------------------------------------------------------
// Debug Output Doesn't Leak Secrets
// ---------------------------------------------------------------------------

/// Validates: Debug formatting of credential types never exposes secret_access_key
/// values, preventing credential leakage via logging.
///
/// **Expected:** FAILS — `CredentialMode` derives `#[derive(Debug)]` which prints
/// all fields verbatim; the fix is a manual impl that redacts secrets.
#[test]
fn debug_output_does_not_contain_secret_key() {
    let creds = Credentials {
        region: "us-east-1".to_string(),
        host_identifier: "arn:aws:iam::123456789012:user/test".to_string(),
        mode: CredentialMode::IamUser {
            access_key_id: "AKIAIOSFODNN7EXAMPLE".to_string(),
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY".to_string(),
        },
    };

    let debug_output = format!("{creds:?}");

    assert!(
        !debug_output.contains("wJalrXUtnFEMI"),
        "Debug output must not contain secret access key. Got: {debug_output}"
    );
    assert!(
        !debug_output.contains("EXAMPLEKEY"),
        "Debug output must not contain any part of the secret. Got: {debug_output}"
    );
}

// ---------------------------------------------------------------------------
// Core Dump Protection
// ---------------------------------------------------------------------------

/// Validates: Core dumps are disabled for the agent process, preventing
/// extraction of AWS credentials from crash artifacts.
///
/// **Expected:** PASSES — `daemon::core_dumps::disable()` sets
/// `RLIMIT_CORE = 0 0` and (on Linux) `PR_SET_DUMPABLE = 0`. This test
/// exercises the agent's actual suppression routine and inspects both
/// the nix-level view (`getrlimit`) and the kernel view (`/proc/self/limits`)
/// so a regression in either layer is caught.
#[test]
#[cfg(target_os = "linux")]
fn core_dumps_disabled() {
    use codedeploy_agent::daemon::core_dumps;
    use nix::sys::prctl;
    use nix::sys::resource::{Resource, getrlimit};

    // Run the agent's actual suppression routine in this process.
    core_dumps::disable();

    // Layer 1: the nix view of the soft + hard RLIMIT_CORE.
    let (soft, hard) = getrlimit(Resource::RLIMIT_CORE).expect("getrlimit RLIMIT_CORE");
    assert_eq!(soft, 0, "soft RLIMIT_CORE must be 0 after disable(), got {soft}");
    assert_eq!(hard, 0, "hard RLIMIT_CORE must be 0 after disable(), got {hard}");

    // Layer 2: the kernel view via /proc/self/limits, so a regression that
    // set the rlimit only in userspace (e.g., on a mocked setrlimit) still
    // trips this test.
    let limits = fs::read_to_string("/proc/self/limits").expect("read /proc/self/limits");
    let core_line = limits
        .lines()
        .find(|l| l.starts_with("Max core file size"))
        .expect("/proc/self/limits has no 'Max core file size' row");
    // Format: "Max core file size        <soft>       <hard>        <units>"
    // Index from the end so header-word count changes don't shift values.
    let cols: Vec<&str> = core_line.split_whitespace().collect();
    let n = cols.len();
    assert!(n >= 3, "unexpected /proc/self/limits row: {core_line:?}");
    assert_eq!(cols[n - 1], "bytes", "expected 'bytes' unit in {core_line:?}");
    assert_eq!(cols[n - 2], "0", "kernel hard core limit must be 0, got {}", cols[n - 2]);
    assert_eq!(cols[n - 3], "0", "kernel soft core limit must be 0, got {}", cols[n - 3]);

    // Layer 3: PR_SET_DUMPABLE — defense-in-depth against ptrace and
    // /proc/<pid>/mem reads even if RLIMIT_CORE is somehow raised.
    let dumpable = prctl::get_dumpable().expect("prctl get_dumpable");
    assert!(!dumpable, "process must not be dumpable after disable()");
}

// ===========================================================================
// Token Security
// ===========================================================================

// ---------------------------------------------------------------------------
// Token File Permissions
// ---------------------------------------------------------------------------

/// Validates: Command port discovery file permissions are restricted to 0600
/// (owner read/write only), preventing non-root users from reading the
/// authentication token.
///
/// **Expected:** PASSES — `set_permissions()` in `auth.rs` explicitly sets `0o600`.
#[cfg(unix)]
#[test]
fn token_file_permissions_are_0600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().expect("create temp dir");
    let path = dir.path().join("state/.command-port");
    let _auth = Auth::init(path.clone(), 12345).expect("init Auth");

    let mode = fs::metadata(&path).expect("read token file metadata").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "Token file should be 0600, got {mode:04o}");
}

// ---------------------------------------------------------------------------
// Invalid Token Rejection
// ---------------------------------------------------------------------------

/// Validates: Command port rejects all invalid token variants — empty string,
/// wrong token, partial match, and token with appended bytes — via
/// constant-time comparison.
///
/// **Expected:** PASSES — `constant_time_eq()` rejects length mismatches and
/// content mismatches.
#[test]
fn rejects_all_invalid_token_variants() {
    let dir = TempDir::new().expect("create temp dir");
    let path = dir.path().join(".cp");
    let auth = Auth::init(path.clone(), 1).expect("init Auth");

    // Read the real token from the discovery file
    let content = fs::read_to_string(&path).expect("read discovery file");
    let parsed: serde_json::Value =
        serde_json::from_str(&content).expect("parse discovery file JSON");
    let real_token = parsed["token"].as_str().expect("token field must be a string").to_string();

    // Build invalid token variants
    let all_zeros = "0".repeat(64);
    let all_fs = "f".repeat(64);
    let partial = &real_token[..32];
    let with_extra = format!("{real_token}extra");

    let invalid_tokens: Vec<(&str, &str)> = vec![
        ("", "empty token"),
        ("wrong_token_value", "completely wrong token"),
        (partial, "partial token (first half)"),
        (&with_extra, "token with extra bytes"),
        (&all_zeros, "all-zeros token"),
        (&all_fs, "all-f's token"),
    ];

    for (token, description) in &invalid_tokens {
        assert!(!auth.validate(token), "Should reject {description}: '{token}'");
    }

    // Verify the real token still works
    assert!(auth.validate(&real_token), "Should accept the real token");
}

// ---------------------------------------------------------------------------
// Kernel-Level Localhost Binding (/proc/net/tcp)
// ---------------------------------------------------------------------------

/// Validates: At the kernel level, the command port socket is bound to
/// 127.0.0.1 (0100007F in hex) and NOT to 0.0.0.0 (00000000).
/// This is ground-truth verification from the Linux kernel's TCP socket table.
///
/// **Expected:** PASSES — `server::bind()` binds to `127.0.0.1:0`, which the
/// kernel records as local address `0100007F` in `/proc/net/tcp`.
#[cfg(target_os = "linux")]
#[test]
fn command_port_binds_to_localhost_proc_net_tcp() {
    use codedeploy_agent::command_port::server;

    // Start the command port listener — this creates a real TCP socket in LISTEN state
    let (_listener, port) = server::bind().expect("Failed to bind command port");

    // Convert port to uppercase hex as it appears in /proc/net/tcp
    let port_hex = format!("{port:04X}");

    // Read the kernel's TCP socket table
    let proc_tcp = fs::read_to_string("/proc/net/tcp")
        .expect("Failed to read /proc/net/tcp — test requires Linux");

    // Expected entry: local address "0100007F:<port_hex>" (127.0.0.1 in little-endian hex)
    let localhost_binding = format!("0100007F:{port_hex}");
    let wildcard_binding = format!("00000000:{port_hex}");

    let has_localhost = proc_tcp.lines().any(|line| line.contains(&localhost_binding));
    let has_wildcard = proc_tcp.lines().any(|line| line.contains(&wildcard_binding));

    assert!(
        has_localhost,
        "Command port (port {port}) must appear as 0100007F:{port_hex} (127.0.0.1) in /proc/net/tcp.\n\
         Searched for: '{localhost_binding}'\n\
         /proc/net/tcp contents:\n{proc_tcp}"
    );
    assert!(
        !has_wildcard,
        "Command port (port {port}) must NOT appear as 00000000:{port_hex} (0.0.0.0) in /proc/net/tcp.\n\
         Found wildcard binding which means the port is exposed to all interfaces."
    );
}

// ===========================================================================
// Property-Based Tests
// ===========================================================================

use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Property 23: Invalid Token Rejection
// ---------------------------------------------------------------------------

// Property 23: Invalid token rejection
// Validates: For any randomly generated string that is not the exact token,
// Auth::validate() returns false — no false positives in token validation.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_rejects_any_non_matching_token(fake_token in "\\PC{0,128}") {
        let dir = TempDir::new().expect("create temp dir for prop test");
        let path = dir.path().join(".cp");
        let auth = Auth::init(path.clone(), 1).expect("init Auth for prop test");

        // Read the real token
        let content = fs::read_to_string(&path).expect("read discovery file");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse discovery JSON");
        let real_token = parsed["token"].as_str().expect("token field must be string").to_string();

        // If the generated string happens to match the real token, skip
        // (astronomically unlikely with 256-bit tokens, but mathematically correct)
        if fake_token != real_token {
            prop_assert!(
                !auth.validate(&fake_token),
                "Should reject non-matching token: '{}'", fake_token
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property: Debug Never Leaks Secret Key
// ---------------------------------------------------------------------------

// Property: Credential Debug safety
// Validates: For any CredentialMode::IamUser containing arbitrary
// secret_access_key values, the Debug output never includes the secret material.
//
// **Expected:** FAILS — `CredentialMode` derives `#[derive(Debug)]` which
// prints all fields verbatim, including `secret_access_key`.
#[cfg(test)]
mod prop_debug_safety {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_debug_never_leaks_secret_key(
            access_key in "[A-Z0-9]{20}",
            secret_key in "[A-Za-z0-9/+=]{40}"
        ) {
            let creds = Credentials {
                region: "us-east-1".to_string(),
                host_identifier: "arn:aws:iam::123456789012:user/test".to_string(),
                mode: CredentialMode::IamUser {
                    access_key_id: access_key.clone(),
                    secret_access_key: secret_key.clone(),
                },
            };

            let debug_output = format!("{creds:?}");
            prop_assert!(
                !debug_output.contains(&secret_key),
                "Debug output must not contain secret_access_key '{}'. Got: {}",
                secret_key, debug_output
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property: Token File Permissions Always 0600
// ---------------------------------------------------------------------------

// Property: Token file permission invariant
// Validates: For any port number used in Auth::init(), the resulting discovery
// file always has mode 0600, regardless of the port or path.
#[cfg(unix)]
mod prop_token_permissions {
    use super::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]

        #[test]
        fn prop_token_file_always_has_0600_permissions(port in 1024..65535u16) {
            use std::os::unix::fs::PermissionsExt;

            let dir = TempDir::new().expect("create temp dir for prop permissions test");
            let path = dir.path().join(format!(".cp-{port}"));
            let _auth = Auth::init(path.clone(), port).expect("init Auth for prop permissions test");

            let mode = fs::metadata(&path).expect("read token file metadata").permissions().mode() & 0o777;
            prop_assert_eq!(
                mode, 0o600,
                "Token file for port {} should be 0600, got {:04o}", port, mode
            );
        }
    }
}
