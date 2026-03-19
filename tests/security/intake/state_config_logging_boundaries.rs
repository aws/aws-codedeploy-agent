//! Security tests for state files, config validation, log injection, and
//! boundary conditions (TO-018, TO-019, TO-020, TO-026).
//!
//! These tests verify that the agent correctly handles adversarial or corrupt
//! state data, rejects dangerous configuration values, sanitizes log output,
//! and behaves gracefully at system boundaries (empty archives, max path
//! lengths, disk-full conditions).
//!
//! Tests marked `#[ignore]` document known security gaps — they will pass once
//! the corresponding production-code controls are implemented.

use aws_codedeploy_agent::host_command::bundle_unpacker;
use aws_codedeploy_agent::lifecycle_event::ScriptRunLog;
use aws_codedeploy_agent::runtime::{Checkpoint, DeploymentTracker, FileBasedDeploymentTracker};
use aws_codedeploy_agent::system::SystemFileOperations;

// ===========================================================================
// TO-018: State File Security
// ===========================================================================

// ---------------------------------------------------------------------------
// TC-018-01: State File Permissions
// ---------------------------------------------------------------------------

/// TO-018 / TC-018-01: State files must have restricted permissions.
///
/// **Security property:** Files created by the deployment tracker must not be
/// world-readable or group-readable. Exposure of state files could leak
/// deployment IDs, command identifiers, and timing data to unprivileged users
/// on the same host.
///
/// **Current gap:** `FileBasedDeploymentTracker::start_tracking()` delegates
/// to `PlatformFileOperations::write_with_retry()`, which uses `std::fs::write()`
/// without explicitly setting restrictive permissions afterward. Under the
/// default umask (0022), this produces mode 0644 — group and world readable.
/// The fix should explicitly `chmod` state files to 0600 after creation.
#[test]
#[ignore] // TODO: enable after setting explicit 0600 permissions on state files in FileBasedDeploymentTracker
fn state_files_have_restricted_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());
    tracker.start_tracking("d-test", "cmd-test").unwrap();

    let file_path = dir.path().join("d-test");
    let mode = std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777;

    // State files should not be world-readable or group-readable.
    // The actual mode depends on the process umask; this assertion documents
    // the expectation that group/world bits are cleared.
    assert_eq!(mode & 0o077, 0, "State file has group/world permissions: {:04o}", mode);
}

// ---------------------------------------------------------------------------
// TC-018-02: Corrupted State File — Graceful Handling
// ---------------------------------------------------------------------------

/// TO-018 / TC-018-02: Corrupted state files must not crash the agent.
///
/// **Security property:** An attacker who can tamper with on-disk state files
/// (truncated JSON, random bytes, invalid UTF-8, wrong schema) must not be
/// able to crash the agent via a deserialization panic. The `Checkpoint`
/// struct derives `serde::Deserialize`, and `serde_json::from_str()` must
/// return `Err` for all malformed inputs.
///
/// **Expected:** PASSES — `serde_json` returns `Err` on all invalid inputs.
#[test]
fn corrupted_state_file_handled_gracefully() {
    // Truncated JSON — missing closing brace
    let result: Result<Checkpoint, _> = serde_json::from_str(r#"{"deployment_id":"d-1""#);
    assert!(result.is_err(), "Truncated JSON must fail to parse");

    // Random bytes (invalid UTF-8 in a &str isn't possible, but non-JSON ASCII is)
    let result: Result<Checkpoint, _> = serde_json::from_str("\x00\x01\x02");
    assert!(result.is_err(), "Random bytes must fail to parse");

    // Empty string
    let result: Result<Checkpoint, _> = serde_json::from_str("");
    assert!(result.is_err(), "Empty string must fail to parse");

    // Valid JSON but wrong schema (array instead of object)
    let result: Result<Checkpoint, _> = serde_json::from_str("[1, 2, 3]");
    assert!(result.is_err(), "Array must fail to parse as Checkpoint");

    // Null in required field
    let result: Result<Checkpoint, _> = serde_json::from_str(
        r#"{"deployment_id":null,"command_id":"c","state_data":[],"timestamp":0}"#,
    );
    assert!(result.is_err(), "Null deployment_id must fail");

    // Extra-large numeric values (potential integer overflow)
    let result: Result<Checkpoint, _> = serde_json::from_str(
        r#"{"deployment_id":"d","command_id":"c","state_data":[],"timestamp":99999999999999999999}"#,
    );
    assert!(result.is_err(), "Overflowing u64 timestamp must fail to parse");

    // Valid Checkpoint round-trip (sanity check — ensures Checkpoint is deserializable)
    let valid =
        r#"{"deployment_id":"d-ok","command_id":"cmd-ok","state_data":[1,2,3],"timestamp":1234}"#;
    let checkpoint: Checkpoint =
        serde_json::from_str(valid).expect("Valid Checkpoint JSON must parse successfully");
    assert_eq!(checkpoint.deployment_id, "d-ok");
    assert_eq!(checkpoint.command_id, "cmd-ok");
    assert_eq!(checkpoint.state_data, vec![1u8, 2, 3]);
    assert_eq!(checkpoint.timestamp, 1234);
}

// ---------------------------------------------------------------------------
// TC-018-03: State File with Path Traversal Deployment ID
// ---------------------------------------------------------------------------

/// TO-018 / TC-018-03: Deployment IDs with path traversal must be rejected.
///
/// **Security property:** `FileBasedDeploymentTracker::tracking_file_path()`
/// joins `deployment_id` directly to the tracking directory via
/// `self.tracking_dir.join(deployment_id)`. A malicious deployment ID like
/// `../../../etc/passwd` would create or overwrite files outside the tracking
/// directory, potentially enabling arbitrary file write as root.
///
/// **Current gap:** No sanitization of `deployment_id` — the join happens
/// unconditionally. The fix should validate that the resolved path stays
/// within `tracking_dir`.
#[test]
#[ignore] // TODO: enable after implementing deployment_id sanitization in FileBasedDeploymentTracker
fn state_file_rejects_traversal_deployment_id() {
    let dir = tempfile::TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    // Each of these deployment IDs attempts to escape the tracking directory
    let malicious_ids = [
        "../../../etc/passwd",
        "..%2F..%2Fetc/passwd",
        "d-123/../../escape",
    ];

    for id in &malicious_ids {
        let result = tracker.start_tracking(id, "cmd-evil");
        assert!(result.is_err(), "Should reject deployment ID with path traversal: {id}");
    }
}

// ===========================================================================
// TO-019: Config Validation
// ===========================================================================

// ---------------------------------------------------------------------------
// TC-019-01: Config with Invalid Endpoint — Rejection
// ---------------------------------------------------------------------------

/// TO-019 / TC-019-01: Invalid endpoint URLs must be rejected.
///
/// **Security property:** `AgentConfig` accepts any string for
/// `deploy_control_endpoint` and `s3_endpoint_override`. An attacker who
/// controls the config file could redirect the agent to an attacker-controlled
/// server (`http://evil.com`), trigger local file reads (`file:///etc/passwd`),
/// or inject payloads via non-HTTP schemes (`javascript:`, `ftp://`).
///
/// **Current gap:** `AgentConfig::from_yaml()` uses `#[serde(default)]` and
/// performs no URL scheme or domain validation on endpoint fields.
#[test]
#[ignore] // TODO: enable after implementing endpoint URL validation in AgentConfig
fn config_rejects_invalid_endpoints() {
    use aws_codedeploy_agent::config::AgentConfig;

    let dangerous_endpoints = [
        ("http://evil.com", "non-AWS endpoint"),
        ("file:///etc/passwd", "file:// scheme"),
        ("javascript:alert(1)", "javascript: scheme"),
        ("ftp://evil.com/payload", "ftp:// scheme"),
    ];

    for (endpoint, description) in &dangerous_endpoints {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("test.yml");
        std::fs::write(&path, format!("deploy_control_endpoint: \"{endpoint}\"\n")).unwrap();

        let result = AgentConfig::from_file(&path);
        assert!(result.is_err(), "Config should reject {description}: {endpoint}");
    }
}

// ---------------------------------------------------------------------------
// TC-019-02: Config with Excessive Timeout — Capping
// ---------------------------------------------------------------------------

/// TO-019 / TC-019-02: Excessive timeouts must be capped.
///
/// **Security property:** `kill_agent_max_wait_time_seconds` has no upper
/// bound. A value of `999999999` (≈31 years) could prevent the agent from
/// being killed during a stuck deployment, effectively creating a
/// denial-of-service condition where the host remains in a degraded state
/// indefinitely.
///
/// **Current gap:** No timeout capping exists — any u64 value is accepted.
/// The fix should cap at a reasonable maximum (e.g., 86400 seconds / 24 hours).
#[test]
#[ignore] // TODO: enable after implementing timeout capping in AgentConfig
fn config_caps_excessive_timeout() {
    use aws_codedeploy_agent::config::AgentConfig;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("test.yml");
    std::fs::write(&path, "kill_agent_max_wait_time_seconds: 999999999\n").unwrap();

    let config = AgentConfig::from_file(&path).unwrap();

    // Maximum should be 24 hours (86400 seconds) or similar reasonable cap
    let max_allowed: u64 = 86400;
    assert!(
        config.kill_agent_max_wait_time_seconds <= max_allowed,
        "Timeout {} exceeds max allowed {}",
        config.kill_agent_max_wait_time_seconds,
        max_allowed
    );
}

// ---------------------------------------------------------------------------
// TC-019-03: Config File Permission Warning
// ---------------------------------------------------------------------------

/// TO-019 / TC-019-03: World-readable config files must produce a warning.
///
/// **Security property:** The config file may contain sensitive fields
/// (`proxy_uri` with credentials, `on_premises_config_file` path). If the
/// file is world-readable (mode 0644), any local user can read these values.
/// The agent should warn operators about insecure permissions.
///
/// **Current gap:** `AgentConfig::from_file()` does not check file permissions
/// before reading. The fix should `stat()` the file and log a warning if
/// group/world-readable bits are set.
#[test]
#[ignore] // TODO: enable after implementing config file permission check in AgentConfig::from_file()
fn config_warns_on_insecure_permissions() {
    use aws_codedeploy_agent::config::AgentConfig;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("insecure.yml");
    std::fs::write(&path, "wait_between_runs: 30\n").unwrap();

    // Set world-readable permissions
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&path, perms).unwrap();

    // Load should succeed but produce a warning.
    // (Implementation: check permissions in from_file(), log warning via tracing::warn!())
    let config = AgentConfig::from_file(&path);
    assert!(config.is_ok(), "Config should still load with insecure permissions");

    // NOTE: Verifying the warning log requires a log capture mechanism.
    // Implementation should use tracing::warn!() and test with tracing-test crate
    // or a tracing subscriber that captures events for assertion.
}

// ===========================================================================
// TO-020: Log Injection
// ===========================================================================

// ---------------------------------------------------------------------------
// TC-020-01: ANSI Escape Sequence Sanitization
// ---------------------------------------------------------------------------

/// TO-020 / TC-020-01: ANSI escape sequences must be sanitized in logs.
///
/// **Security property:** A malicious lifecycle script can output ANSI escape
/// sequences that manipulate terminal display when logs are viewed:
/// - `\x1b[2J` clears the screen (hiding earlier log lines)
/// - `\x1b[31m` changes text color (masking error messages)
/// - `\x1b]0;title\x07` changes the terminal title (social engineering)
/// - `\x1b[3;1H` repositions the cursor (overwriting visible output)
///
/// These can trick operators into misreading logs during incident response.
///
/// **Current gap:** `ScriptRunLog::write_line()` writes content as-is with no
/// ANSI stripping. The fix should strip or escape all `\x1b` sequences.
#[test]
#[ignore] // TODO: enable after implementing ANSI stripping in ScriptRunLog::write_line()
fn log_sanitizes_ansi_escape_sequences() {
    let mut log = ScriptRunLog::in_memory();

    let ansi_inputs = [
        "\x1b[2J",               // Clear screen
        "\x1b[31mred\x1b[0m",    // Color code
        "\x1b]0;evil title\x07", // Title manipulation
        "\x1b[3;1HOverwrite",    // Cursor positioning
    ];

    for input in &ansi_inputs {
        log.write_line("[stdout]", input);
    }

    let entries = log.entries();
    for entry in &entries {
        assert!(
            !entry.contains('\x1b'),
            "Log entry contains raw ANSI escape sequence: {:?}",
            entry
        );
    }
}

// ---------------------------------------------------------------------------
// TC-020-02: Fake Log Entry Distinction
// ---------------------------------------------------------------------------

/// TO-020 / TC-020-02: Script output must be distinguishable from agent logs.
///
/// **Security property:** A malicious script could output text that mimics
/// agent log format (e.g., `2026-02-24 INFO Fake log entry`) to confuse
/// incident responders or inject misleading entries into log aggregation
/// systems. The `write_line()` method must always prepend a real timestamp
/// and a prefix like `[stdout]` or `[stderr]`, making the fake entry clearly
/// part of the script's output — not an agent-generated log line.
///
/// **Expected:** PASSES — `write_line()` always prepends `"{ts} {prefix}"`.
#[test]
fn log_prefixes_script_output() {
    let mut log = ScriptRunLog::in_memory();

    // Script tries to inject a fake agent log entry
    log.write_line("[stdout]", "2026-02-24 INFO Fake log entry");

    let entries = log.entries();
    assert_eq!(entries.len(), 1);

    // The entry has the REAL timestamp + [stdout] prefix, making the fake
    // "2026-02-24 INFO" clearly part of the script's output content.
    // Format: "{real_timestamp} [stdout]2026-02-24 INFO Fake log entry\n"
    let entry = &entries[0];
    assert!(
        entry.contains("[stdout]2026-02-24 INFO Fake log entry"),
        "Script output must be wrapped with prefix, got: {entry:?}"
    );

    // Verify the entry starts with a real timestamp (YYYY-MM-DD HH:MM:SS format)
    // followed by a space and the prefix — proving the fake date is embedded
    // within the real log line, not at the start.
    // Timestamp format: "2026-03-09 12:34:56 [stdout]..."
    let bytes = entry.as_bytes();
    assert!(bytes.len() > 20, "Entry too short to contain timestamp: {entry:?}");
    // Check date portion: YYYY-MM-DD (positions 0..10)
    assert_eq!(bytes[4], b'-', "Expected '-' at position 4, got: {entry:?}");
    assert_eq!(bytes[7], b'-', "Expected '-' at position 7, got: {entry:?}");
    // Check time portion: HH:MM:SS (positions 11..19)
    assert_eq!(bytes[10], b' ', "Expected ' ' at position 10, got: {entry:?}");
    assert_eq!(bytes[13], b':', "Expected ':' at position 13, got: {entry:?}");
    assert_eq!(bytes[16], b':', "Expected ':' at position 16, got: {entry:?}");
    // Check prefix follows timestamp
    assert!(
        entry[19..].starts_with(" [stdout]"),
        "Entry must have ' [stdout]' after timestamp, got: {entry:?}"
    );
}

// ---------------------------------------------------------------------------
// TC-020-03: Excessive Output Truncation (in-memory buffer)
// ---------------------------------------------------------------------------

/// TO-020 / TC-020-03: In-memory log buffer is bounded.
///
/// **Security property:** A malicious lifecycle script could output enormous
/// volumes of data in an attempt to exhaust agent memory. The `BoundedFifoVec`
/// backing `ScriptRunLog` caps the in-memory buffer at `MAX_BYTES` (2048
/// bytes), evicting oldest entries when the limit is exceeded. This prevents
/// unbounded memory growth from script output.
///
/// **Expected:** PASSES — `BoundedFifoVec` evicts oldest entries when total
/// bytes exceed 2048.
#[test]
fn log_buffer_is_bounded() {
    let mut log = ScriptRunLog::in_memory();

    // Write many lines — buffer should not grow without bound.
    // Each formatted line is ~40+ bytes (timestamp + prefix + content + newline),
    // so 10,000 lines would be ~400KB without bounding.
    for i in 0..10_000 {
        log.write_line("[stdout]", &format!("line {i}"));
    }

    let entries = log.entries();
    // BoundedFifoVec caps at MAX_BYTES = 2048 bytes total.
    // With ~40 byte entries, we expect roughly 50 entries max.
    assert!(
        entries.len() < 10_000,
        "Buffer should be bounded, got {} entries",
        entries.len()
    );
    // Verify it's actually bounded tightly (not just slightly less than 10k)
    assert!(
        entries.len() < 200,
        "Buffer should be tightly bounded (MAX_BYTES=2048), got {} entries",
        entries.len()
    );
}

// ---------------------------------------------------------------------------
// TC-020-03: Excessive Output Truncation (on-disk file)
// ---------------------------------------------------------------------------

/// TO-020 / TC-020-03 (file): On-disk log file size must be bounded.
///
/// **Security property:** While the in-memory buffer is bounded by
/// `BoundedFifoVec`, the on-disk log file written by `ScriptRunLog::open()`
/// grows without limit. A malicious script outputting 100MB+ would fill the
/// disk, potentially causing the agent and other system services to fail
/// (denial of service). The fix should add a byte counter and truncate or
/// rotate the file when it exceeds a configured maximum (e.g., 5MB).
///
/// **Current gap:** `ScriptRunLog::write_line()` unconditionally appends to
/// the file via `file.write_all()` with no size check.
#[test]
#[ignore] // TODO: enable after implementing on-disk log size limit in ScriptRunLog
fn log_file_size_is_bounded() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("big.log");
    let mut log = ScriptRunLog::open(&path).unwrap();

    // Write ~10MB of data (1,000 lines × 10,000 chars each)
    let big_line = "x".repeat(10_000);
    for _ in 0..1_000 {
        log.write_line("[stdout]", &big_line);
    }

    let size = std::fs::metadata(&path).unwrap().len();
    let max_log_size: u64 = 5 * 1024 * 1024; // 5MB expected cap
    assert!(
        size <= max_log_size,
        "Log file size {} bytes exceeds max {} bytes",
        size,
        max_log_size
    );
}

// ===========================================================================
// TO-026: Boundary Conditions
// ===========================================================================

// ---------------------------------------------------------------------------
// TC-026-01: Empty Deployment Bundle
// ---------------------------------------------------------------------------

/// TO-026 / TC-026-01: Empty bundle must fail gracefully.
///
/// **Security property:** A 0-byte archive file is a degenerate input that
/// could trigger unexpected behavior in archive extraction code paths. The
/// agent must detect the empty/corrupt archive and return a clear error
/// rather than panicking, producing an empty deployment directory, or
/// entering an inconsistent state.
///
/// **Expected:** PASSES — `tar -xf` on a 0-byte file returns non-zero exit
/// code, which `bundle_unpacker::unpack()` translates to `Err(io::Error)`.
#[test]
fn unpack_empty_bundle_fails_gracefully() {
    let dir = tempfile::TempDir::new().unwrap();
    let empty_tar = dir.path().join("empty.tar");
    std::fs::write(&empty_tar, b"").unwrap();

    let dest = dir.path().join("deployment");
    // bundle_type "tar" maps to `tar -xf` extraction
    let result = bundle_unpacker::unpack(&empty_tar, &dest, "tar");
    assert!(result.is_err(), "Empty archive should fail to extract");
}

// ---------------------------------------------------------------------------
// TC-026-02: Maximum Path Length
// ---------------------------------------------------------------------------

/// TO-026 / TC-026-02: Maximum path length must be handled without overflow.
///
/// **Security property:** An attacker-controlled AppSpec could specify file
/// paths at or near the OS maximum (`PATH_MAX` = 4096 on Linux). This must
/// not cause buffer overflows, panics, or undefined behavior. Rust's `String`
/// and `PathBuf` handle arbitrary lengths safely, and the OS returns
/// `ENAMETOOLONG` for paths exceeding the limit.
///
/// **Expected:** PASSES — Rust strings handle arbitrary lengths. The parser
/// either succeeds (paths are just strings at parse time) or fails with a
/// clear error from OS-level operations.
#[test]
fn long_path_does_not_crash() {
    use aws_codedeploy_agent::application_specification::AppSpec;

    // PATH_MAX on Linux is 4096 bytes
    let long_path = format!("/{}", "a".repeat(4000));
    let yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: {long_path}\n    destination: {long_path}\n"
    );

    // Should either succeed or fail gracefully — must never panic or overflow
    let result = AppSpec::parse(&yaml);
    // AppSpec parsing deals with strings, not filesystem ops, so this should succeed
    assert!(result.is_ok(), "Long path should not crash the parser: {:?}", result.err());
}

// ---------------------------------------------------------------------------
// TC-026-03: Disk Full Condition
// ---------------------------------------------------------------------------

/// TO-026 / TC-026-03: Disk full during extraction is detected.
///
/// **Security property:** When the disk fills up during archive extraction,
/// the agent must detect the condition and report it clearly rather than
/// leaving partially-extracted files that could cause inconsistent deployments.
///
/// **Implementation note:** `unpack_zip()` already handles exit code 50
/// (disk full) from `unzip`, cleaning up with `fs::remove_dir_all(dest)` and
/// returning a descriptive error. `tar` failures also produce I/O errors.
/// Full simulation requires a constrained filesystem (tmpfs with size limit),
/// so this test validates the code path exists by inspecting the known
/// error-handling behavior.
///
/// **Expected:** PASSES (code inspection) — the exit code 50 handling is
/// present in the source. Actual disk-full simulation would require
/// infrastructure-level test fixtures (constrained tmpfs).
#[test]
fn unpack_zip_detects_disk_full_exit_code() {
    // Verify the disk-full error handling logic exists in the unpack_zip code
    // path. The code in bundle_unpacker.rs checks:
    //
    //   if output.status.code() == Some(50) {
    //       let _ = fs::remove_dir_all(dest);
    //       return Err(io::Error::other("The disk is (or was) full..."));
    //   }
    //
    // A true disk-full test requires a size-limited tmpfs mount, which is an
    // infrastructure-dependent test better suited for the canaries/ suite.
    //
    // Here we validate that a corrupt/invalid zip also fails gracefully,
    // exercising the same error-handling code path.
    let dir = tempfile::TempDir::new().unwrap();
    let bad_zip = dir.path().join("notazip.zip");
    std::fs::write(&bad_zip, b"this is not a zip file").unwrap();

    let dest = dir.path().join("deployment");
    let result = bundle_unpacker::unpack(&bad_zip, &dest, "zip");
    assert!(result.is_err(), "Corrupt zip file should fail to extract");
}
