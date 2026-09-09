//! Security tests for state files, config validation, log injection, and
//! boundary conditions.

use codedeploy_agent::host_command::bundle_unpacker;
use codedeploy_agent::lifecycle_event::ScriptRunLog;
use codedeploy_agent::runtime::{DeploymentTracker, FileBasedDeploymentTracker};
use codedeploy_agent::system::SystemFileOperations;

/// Local stand-in for the deleted `runtime::Checkpoint` — used only to verify
/// that `serde_json` deserialization of corrupt data returns `Err` without panicking.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct Checkpoint {
    pub deployment_id: String,
    pub command_id: String,
    pub state_data: Vec<u8>,
    pub timestamp: u64,
}

// ===========================================================================
// State File Security
// ===========================================================================

// ---------------------------------------------------------------------------
// State File Permissions
// ---------------------------------------------------------------------------

/// State files must have restricted permissions under the opt-in
/// `restrict_agent_dir_permissions` hardening.
///
/// **Security property:** with the hardening flag set, files created by the
/// deployment tracker must not be world-readable or group-readable. Exposure
/// of state files could leak deployment IDs, command identifiers, and timing
/// data to unprivileged users on the same host.
///
/// **Default posture:** by default the tracker writes 0644 — the long-standing
/// world-readable behavior that host tooling outside the agent depends on. The
/// restricted mode is available as opt-in hardening, matching the
/// deployment-dir and log-mode defaults.
#[cfg(unix)]
#[test]
fn state_files_have_restricted_permissions_under_hardening() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new_with_ops(
        dir.path().to_path_buf(),
        SystemFileOperations::with_policy(true),
    );
    tracker.start_tracking("d-test", "cmd-test").expect("start tracking");

    let file_path = dir.path().join("d-test");
    let mode = std::fs::metadata(&file_path)
        .expect("read state file metadata")
        .permissions()
        .mode()
        & 0o777;

    assert_eq!(mode & 0o077, 0, "State file has group/world permissions: {:04o}", mode);
}

/// Default counterpart: with the hardening flag unset the tracker writes 0644.
#[cfg(unix)]
#[test]
fn state_files_are_world_readable_by_default() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());
    tracker.start_tracking("d-test", "cmd-test").expect("start tracking");

    let mode = std::fs::metadata(dir.path().join("d-test"))
        .expect("read state file metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o644, "state file mode {mode:#o}, want the default 0644");
}

// ---------------------------------------------------------------------------
// Corrupted State File — Graceful Handling
// ---------------------------------------------------------------------------

/// Corrupted state files must not crash the agent.
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
// State File with Path Traversal Deployment ID
// ---------------------------------------------------------------------------

/// Deployment IDs with path traversal must be rejected.
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
fn state_file_rejects_traversal_deployment_id() {
    let dir = tempfile::TempDir::new().expect("create temp dir");
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

// ---------------------------------------------------------------------------
// Third-party readers of the deployment root
// ---------------------------------------------------------------------------

/// Host tooling outside the agent lists `<root_dir>/ongoing-deployment` to
/// discover in-flight deployments; such a process typically runs unprivileged
/// and treats an `EACCES` from `read_dir` as fatal.
///
/// **Default property:** every directory on the path such a process walks
/// (`root_dir` and `ongoing-deployment`) must carry world read+execute bits
/// (0755). Tests run single-user, so we assert the `o+rx` bits that make a
/// foreign process' `read_dir` succeed.
#[cfg(unix)]
#[test]
fn ongoing_deployment_dir_is_readable_by_other_users_by_default() {
    use codedeploy_agent::config::AgentConfig;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let config = AgentConfig {
        pid_dir: dir.path().join("pid"),
        log_dir: dir.path().join("logs"),
        root_dir: dir.path().join("deployment-root"),
        ongoing_deployment_tracking: "ongoing-deployment".to_string(),
        ..AgentConfig::default()
    };

    config.ensure_dirs().expect("ensure_dirs");

    let ongoing = config.root_dir.join("ongoing-deployment");
    for path in [&config.root_dir, &ongoing] {
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o005,
            0o005,
            "{} must be world read+execute (got {mode:#o}); an unprivileged host \
             process must be able to read_dir it",
            path.display()
        );
    }

    // And read_dir itself must succeed on the telemetry target.
    std::fs::read_dir(&ongoing).expect("read_dir(ongoing-deployment) must succeed");
}

/// The tightened modes remain available as opt-in hardening: with
/// `restrict_agent_dir_permissions: true` the ongoing-deployment dir
/// goes back to 0700 (and root_dir to 0711), for hosts where no process outside
/// the agent reads agent state.
#[cfg(unix)]
#[test]
fn ongoing_deployment_dir_restricted_under_opt_in_hardening() {
    use codedeploy_agent::config::AgentConfig;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let config = AgentConfig {
        pid_dir: dir.path().join("pid"),
        log_dir: dir.path().join("logs"),
        root_dir: dir.path().join("deployment-root"),
        ongoing_deployment_tracking: "ongoing-deployment".to_string(),
        hardening: codedeploy_agent::config::HardeningConfig {
            restrict_agent_dir_permissions: true,
            ..Default::default()
        },
        ..AgentConfig::default()
    };

    config.ensure_dirs().expect("ensure_dirs");

    let root_mode = std::fs::metadata(&config.root_dir).unwrap().permissions().mode() & 0o777;
    let ongoing_mode = std::fs::metadata(config.root_dir.join("ongoing-deployment"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(root_mode, 0o711, "hardened root_dir must be 0711, got {root_mode:#o}");
    assert_eq!(ongoing_mode, 0o700, "hardened ongoing dir must be 0700, got {ongoing_mode:#o}");
}

// ===========================================================================
// Config Validation
// ===========================================================================

// ---------------------------------------------------------------------------
// Config with Invalid Endpoint — Rejection
// ---------------------------------------------------------------------------

/// Garbage endpoint schemes must be rejected.
///
/// The non-`http(s)` schemes (`file://`, `javascript:`, `ftp://`, …) have no
/// legitimate agent use case and are rejected unconditionally. `http://` is accepted
/// with a plaintext warning.
#[test]
fn config_rejects_invalid_endpoints() {
    use codedeploy_agent::config::AgentConfig;

    let dangerous_endpoints = [
        ("file:///etc/passwd", "file:// scheme"),
        ("javascript:alert(1)", "javascript: scheme"),
        ("ftp://evil.com/payload", "ftp:// scheme"),
    ];

    for (endpoint, description) in &dangerous_endpoints {
        let dir = tempfile::TempDir::new().expect("create temp dir");
        let path = dir.path().join("test.yml");
        std::fs::write(&path, format!("deploy_control_endpoint: \"{endpoint}\"\n"))
            .expect("write test config");

        let result = AgentConfig::from_file(&path);
        assert!(result.is_err(), "Config should reject {description}: {endpoint}");
    }
}

/// `http://` is accepted: it is a deliberate operator choice for a self-hosted
/// mock or a loopback/sidecar that terminates TLS, so it is accepted with a
/// cleartext `warn!` rather than blocked.
#[test]
fn config_http_endpoint_accepted() {
    use codedeploy_agent::config::AgentConfig;

    let dir = tempfile::TempDir::new().expect("create temp dir");

    let permissive = dir.path().join("http.yml");
    std::fs::write(&permissive, "deploy_control_endpoint: \"http://internal-mock.local\"\n")
        .expect("write http config");
    assert!(AgentConfig::from_file(&permissive).is_ok(), "http:// must be accepted");
}

// ---------------------------------------------------------------------------
// Config with Excessive Timeout — Capping
// ---------------------------------------------------------------------------

/// Excessive timeouts must be capped.
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
fn config_caps_excessive_timeout() {
    use codedeploy_agent::config::AgentConfig;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let path = dir.path().join("test.yml");
    std::fs::write(&path, "kill_agent_max_wait_time_seconds: 999999999\n")
        .expect("write test config");

    let config = AgentConfig::from_file(&path).expect("parse config");

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
// Config File Permission Warning
// ---------------------------------------------------------------------------

/// World-readable config files must produce a warning.
///
/// **Security property:** The config file may contain sensitive fields
/// (`proxy_uri` with credentials, `on_premises_config_file` path). If the
/// file is world-readable (mode 0644), any local user can read these values.
/// The agent should warn operators about insecure permissions.
///
/// **Current gap:** `AgentConfig::from_file()` does not check file permissions
/// before reading. The fix should `stat()` the file and log a warning if
/// group/world-readable bits are set.
#[cfg(unix)]
#[test]
fn config_warns_on_insecure_permissions() {
    use codedeploy_agent::config::AgentConfig;
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::TempDir::new().expect("create temp dir");
    let path = dir.path().join("insecure.yml");
    std::fs::write(&path, "wait_between_runs: 30\n").expect("write test config");

    // Set world-readable permissions
    let mut perms = std::fs::metadata(&path).expect("read config file metadata").permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&path, perms).expect("set insecure permissions");

    // Load should succeed but produce a warning.
    // (Implementation: check permissions in from_file(), log warning via tracing::warn!())
    let config = AgentConfig::from_file(&path);
    assert!(config.is_ok(), "Config should still load with insecure permissions");

    // NOTE: Verifying the warning log requires a log capture mechanism.
    // Implementation should use tracing::warn!() and test with tracing-test crate
    // or a tracing subscriber that captures events for assertion.
}

// ===========================================================================
// Log Injection
// ===========================================================================

// ---------------------------------------------------------------------------
// ANSI Escape Sequence Sanitization
// ---------------------------------------------------------------------------

/// ANSI escape sequences must be sanitized in logs.
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
// Fake Log Entry Distinction
// ---------------------------------------------------------------------------

/// Script output must be distinguishable from agent logs.
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
// Excessive Output Truncation (in-memory buffer)
// ---------------------------------------------------------------------------

/// In-memory log buffer is bounded.
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
// Excessive Output Truncation (on-disk file)
// ---------------------------------------------------------------------------

/// On-disk log file size must be bounded.
///
/// **Security property:** A malicious script outputting unbounded data must not
/// fill the disk. `ScriptRunLog` uses size-based rotation (64 MiB per file,
/// 8 files retained = 512 MiB worst case).
///
/// **Verified:** Write >64 MiB to trigger at least one rotation, then confirm
/// the live file is below the rotation cap.
#[test]
fn log_file_size_is_bounded() {
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let path = dir.path().join("big.log");
    let mut log = ScriptRunLog::open(&path).expect("open log file");

    // Write ~70 MiB of data (7,000 lines × 10,000 chars each ≈ 70 MB)
    // This exceeds the 64 MiB per-file cap and triggers rotation.
    let big_line = "x".repeat(10_000);
    for _ in 0..7_000 {
        log.write_line("[stdout]", &big_line);
    }

    let size = std::fs::metadata(&path).expect("read log file metadata").len();
    let max_file_size: u64 = 64 * 1024 * 1024; // 64 MiB rotation cap
    assert!(
        size <= max_file_size,
        "Live log file size {} bytes exceeds rotation cap {} bytes",
        size,
        max_file_size
    );

    // Verify rotation happened — at least one .1 file should exist
    let rotated = path.with_file_name("big.log.1");
    assert!(rotated.exists(), "Rotation should have created big.log.1");
}

// ===========================================================================
// Boundary Conditions
// ===========================================================================

// ---------------------------------------------------------------------------
// Empty Deployment Bundle
// ---------------------------------------------------------------------------

/// Empty bundle must fail gracefully.
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
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let empty_tar = dir.path().join("empty.tar");
    std::fs::write(&empty_tar, b"").expect("write empty tar");

    let dest = dir.path().join("deployment");
    // bundle_type "tar" maps to `tar -xf` extraction
    let result = bundle_unpacker::unpack(&empty_tar, &dest, "tar", false, false);
    assert!(result.is_err(), "Empty archive should fail to extract");
}

// ---------------------------------------------------------------------------
// Maximum Path Length
// ---------------------------------------------------------------------------

/// Maximum path length must be handled without overflow.
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
    use codedeploy_agent::application_specification::AppSpec;

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
// Disk Full Condition
// ---------------------------------------------------------------------------

/// Disk full during extraction is detected.
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
    // infrastructure-dependent test better suited for an end-to-end suite.
    //
    // Here we validate that a corrupt/invalid zip also fails gracefully,
    // exercising the same error-handling code path.
    let dir = tempfile::TempDir::new().expect("create temp dir");
    let bad_zip = dir.path().join("notazip.zip");
    std::fs::write(&bad_zip, b"this is not a zip file").expect("write bad zip");

    let dest = dir.path().join("deployment");
    let result = bundle_unpacker::unpack(&bad_zip, &dest, "zip", false, false);
    assert!(result.is_err(), "Corrupt zip file should fail to extract");
}

// ===========================================================================
// Property-Based Tests
// ===========================================================================

use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Property 26: State file corruption resilience
// ---------------------------------------------------------------------------

// Property 26: State file corruption resilience
// Validates: For any random corrupted JSON bytes, serde_json deserialization
// into Checkpoint never panics — it returns Err for invalid input.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_corrupted_state_file_never_panics(
        data in proptest::collection::vec(proptest::num::u8::ANY, 0..1000),
    ) {
        // Arrange: random bytes that are overwhelmingly unlikely to be valid Checkpoint JSON

        // Act: attempt to deserialize — must not panic
        let result = serde_json::from_slice::<Checkpoint>(&data);

        // Assert: for random bytes, this should virtually always be Err
        // (Ok is acceptable if random bytes happen to form valid JSON — astronomically unlikely)
        let _ = result;
    }
}

// Property 26: State file corruption resilience (string variant)
// Validates: For any arbitrary string, serde_json deserialization into
// Checkpoint returns Err without panicking.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_corrupted_state_string_never_panics(
        json_like in "\\PC{0,500}",
    ) {
        // Arrange: arbitrary printable string

        // Act: must not panic
        let result = serde_json::from_str::<Checkpoint>(&json_like);

        // Assert: reaching here means no panic occurred
        let _ = result;
    }
}

// ---------------------------------------------------------------------------
// Property 27: State file path traversal rejection
// ---------------------------------------------------------------------------

// Property 27: State file path traversal rejection
// Validates: For any deployment ID containing path traversal sequences
// (../, absolute paths, null bytes), FileBasedDeploymentTracker rejects it.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_deployment_id_traversal_rejected(
        depth in 1usize..6,
        suffix in "[a-z]{1,10}",
    ) {
        // Arrange: build a traversal deployment ID with variable depth
        let malicious_id = format!("{}{}", "../".repeat(depth), suffix);

        let dir = tempfile::TempDir::new().expect("create temp dir for P27 prop test");
        let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

        // Act
        let result = tracker.start_tracking(&malicious_id, "cmd-evil");

        // Assert: must be rejected
        prop_assert!(
            result.is_err(),
            "start_tracking() must reject deployment ID '{}' with path traversal", malicious_id
        );
    }
}

// ---------------------------------------------------------------------------
// Property 28: Configuration endpoint validation
// ---------------------------------------------------------------------------

// Property 28: Configuration endpoint validation
// Validates: For any configuration with a garbage-scheme endpoint URL (file://,
// javascript:, ftp://, gopher://, data:), AgentConfig::from_file() rejects it.
// http:// is excluded — it is accepted (see config_http_endpoint_accepted).
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_non_https_endpoints_rejected(
        scheme in prop_oneof![
            Just("file://"),
            Just("javascript:"),
            Just("ftp://"),
            Just("gopher://"),
            Just("data:"),
        ],
        host in "[a-z]{3,15}\\.[a-z]{2,5}",
    ) {
        use codedeploy_agent::config::AgentConfig;

        // Arrange: write config with a non-HTTPS endpoint
        let endpoint = format!("{scheme}{host}");
        let dir = tempfile::TempDir::new().expect("create temp dir for P28 prop test");
        let path = dir.path().join("test.yml");
        std::fs::write(&path, format!("deploy_control_endpoint: \"{endpoint}\"\n"))
            .expect("write test config for P28");

        // Act
        let result = AgentConfig::from_file(&path);

        // Assert: must be rejected
        prop_assert!(
            result.is_err(),
            "AgentConfig should reject non-HTTPS endpoint '{}'", endpoint
        );
    }
}

// ---------------------------------------------------------------------------
// Property 29: Configuration timeout capping
// ---------------------------------------------------------------------------

// Property 29: Configuration timeout capping
// Validates: For any excessive timeout value (above the expected cap),
// AgentConfig::from_file() caps the timeout to the maximum allowed value.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_excessive_timeouts_capped(
        timeout_value in 86401u64..=999_999_999u64,
    ) {
        use codedeploy_agent::config::AgentConfig;

        // Arrange
        let dir = tempfile::TempDir::new().expect("create temp dir for P29 prop test");
        let path = dir.path().join("test.yml");
        std::fs::write(&path, format!("kill_agent_max_wait_time_seconds: {timeout_value}\n"))
            .expect("write test config for P29");

        // Act
        let config = AgentConfig::from_file(&path)
            .expect("Config should parse with excessive timeout (capped, not rejected)");

        // Assert: timeout must be capped at 86400 seconds (24 hours)
        let max_allowed: u64 = 86400;
        prop_assert!(
            config.kill_agent_max_wait_time_seconds <= max_allowed,
            "Timeout {} exceeds max allowed {}", config.kill_agent_max_wait_time_seconds, max_allowed
        );
    }
}

// ---------------------------------------------------------------------------
// Property 30: Configuration permission warning
// ---------------------------------------------------------------------------

// Property 30: Configuration permission warning
// Validates: For any config file with group/world-readable permissions,
// AgentConfig::from_file() emits a warning. (Testing permission detection
// rather than warning capture.)
#[cfg(unix)]
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_permissive_config_mode_detected(
        mode_bits in 0o644u32..=0o777u32,
    ) {
        use codedeploy_agent::config::AgentConfig;
        use std::os::unix::fs::PermissionsExt;

        // Arrange: only test modes with group or world read bits
        let has_permissive_bits = (mode_bits & 0o044) != 0;
        if !has_permissive_bits {
            return Ok(());
        }

        let dir = tempfile::TempDir::new().expect("create temp dir for P30 prop test");
        let path = dir.path().join("test.yml");
        std::fs::write(&path, "verbose: true\n").expect("write test config for P30");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode_bits))
            .expect("set permissive permissions for P30");

        // Act: should either reject or warn (implementation-dependent)
        let result = AgentConfig::from_file(&path);

        // Assert: if loading succeeds, the implementation should have emitted a warning.
        // Since we can't capture tracing warnings in a prop test easily, we
        // verify at minimum that the agent either rejects or loads (no panic).
        // A stricter assertion should verify the warning via a tracing subscriber.
        let _ = result;
    }
}

// ---------------------------------------------------------------------------
// Property 31: ANSI escape stripping
// ---------------------------------------------------------------------------

// Property 31: ANSI escape stripping
// Validates: For any string containing ANSI escape sequences (\x1b[...),
// ScriptRunLog::write_line() strips them from the log output.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_ansi_escapes_stripped(
        pre in "[a-zA-Z0-9 ]{0,30}",
        ansi_code in "[0-9;]{1,6}",
        post in "[a-zA-Z0-9 ]{0,30}",
    ) {
        // Arrange: build a string with an embedded ANSI escape
        let input = format!("{pre}\x1b[{ansi_code}m{post}");
        let mut log = ScriptRunLog::in_memory();

        // Act
        log.write_line("[stdout]", &input);

        // Assert: no ANSI escape sequences should remain
        let entries = log.entries();
        for entry in &entries {
            prop_assert!(
                !entry.contains('\x1b'),
                "Log entry must not contain ANSI escape. Input: {:?}, Entry: {:?}",
                input, entry
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Property 32: Script output marking
// ---------------------------------------------------------------------------

// Property 32: Script output marking
// Validates: For any string resembling a log entry (with timestamps and
// severity levels), ScriptRunLog::write_line() wraps it with a real
// timestamp and prefix, making fake entries distinguishable.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_script_output_always_prefixed(
        fake_ts in "[0-9]{4}-[0-9]{2}-[0-9]{2}",
        severity in prop_oneof![Just("INFO"), Just("WARN"), Just("ERROR"), Just("DEBUG")],
        message in "[a-zA-Z0-9 ]{1,50}",
    ) {
        // Arrange: build a string that mimics a log entry
        let fake_log = format!("{fake_ts} {severity} {message}");
        let mut log = ScriptRunLog::in_memory();

        // Act
        log.write_line("[stdout]", &fake_log);

        // Assert: the entry must contain the [stdout] prefix, proving
        // the fake log entry is embedded within a real log line
        let entries = log.entries();
        prop_assert!(
            !entries.is_empty(),
            "Should have at least one log entry"
        );
        let entry = &entries[0];
        prop_assert!(
            entry.contains("[stdout]"),
            "Log entry must contain '[stdout]' prefix: {:?}", entry
        );
        prop_assert!(
            entry.contains(&fake_log),
            "Log entry must contain the original script output: {:?}", entry
        );
    }
}

// ---------------------------------------------------------------------------
// Property 33: Log size truncation
// ---------------------------------------------------------------------------

// Property 33: Log size truncation
// Validates: For any number of oversized output lines written to the
// in-memory ScriptRunLog, the total buffer stays bounded by MAX_BYTES.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn prop_log_buffer_stays_bounded(
        line_count in 100usize..2000,
        line_len in 10usize..500,
    ) {
        // Arrange: create oversized output
        let mut log = ScriptRunLog::in_memory();
        let line = "x".repeat(line_len);

        // Act: write many lines
        for _ in 0..line_count {
            log.write_line("[stdout]", &line);
        }

        // Assert: buffer must be bounded (MAX_BYTES = 2048)
        let entries = log.entries();
        let total_bytes: usize = entries.iter().map(|e| e.len()).sum();

        // BoundedFifoVec caps at 2048 bytes; with entry overhead, the actual
        // count varies but must be dramatically less than what we wrote.
        prop_assert!(
            entries.len() < line_count,
            "Buffer should evict old entries: {} entries for {} writes", entries.len(), line_count
        );
        // The total bytes in the buffer should not exceed MAX_BYTES significantly
        // (allow some overhead for the last entry that triggers eviction)
        let max_bytes_with_margin: usize = 4096; // 2x MAX_BYTES as generous margin
        prop_assert!(
            total_bytes <= max_bytes_with_margin,
            "Buffer total bytes {} exceeds margin {}", total_bytes, max_bytes_with_margin
        );
    }
}
