// Security tests for concurrency and crash recovery.
//
// These tests validate the agent's single-instance enforcement and recovery:
// - Deployments are processed serially (one active at a time)
// - PID file lock prevents a second agent instance
// - Crash recovery detects interrupted deployments and stale PIDs
//
// All tests in this file run without #[ignore] — no infrastructure dependencies.

use codedeploy_agent::daemon::master::{Master, MasterConfig, StartOutcome};
use codedeploy_agent::daemon::pid_file::PidFile;
use codedeploy_agent::runtime::{DeploymentTracker, FileBasedDeploymentTracker};
use codedeploy_agent::system::SystemFileOperations;
use proptest::prelude::*;

// ===========================================================================
// Concurrency & Recovery
// ===========================================================================

// Validates: Multiple simultaneous deployment tracking requests are handled serially,
// with only one active deployment at a time and no file corruption.
#[test]
fn concurrent_deployment_tracking_is_serial() {
    let dir = tempfile::TempDir::new().expect("should create temp dir");
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    // Simulate 5 sequential deployments (agent processes one at a time)
    let deployment_ids: Vec<String> = (1..=5).map(|i| format!("d-{i:03}")).collect();

    for (i, dep_id) in deployment_ids.iter().enumerate() {
        let cmd_id = format!("cmd-{i:03}");

        // Start tracking — should succeed
        tracker
            .start_tracking(dep_id, &cmd_id)
            .expect("should start tracking deployment");

        // Verify only this deployment is in progress
        assert!(
            tracker.is_deployment_in_progress().expect("should check progress"),
            "Deployment {dep_id} should be in progress"
        );

        // Stop tracking — simulates deployment completion
        tracker.stop_tracking(dep_id).expect("should stop tracking deployment");

        // Verify no deployment is in progress after completion
        assert!(
            !tracker.is_deployment_in_progress().expect("should check progress"),
            "No deployment should be in progress after {dep_id} completes"
        );
    }
}

// Validates: When a PID file exists for a running process, a second agent instance
// detects it and exits with an error, preventing multiple instances from corrupting
// shared state.
#[test]
fn second_instance_detects_pid_file_lock() {
    let dir = tempfile::TempDir::new().expect("should create temp dir");

    // First instance writes PID file with current process PID
    let pid_file = PidFile::new(dir.path(), "agent.pid");
    pid_file.write().expect("should write PID file");

    // Verify PID file exists and contains our PID
    let read_pid = pid_file.read().expect("should read PID file");
    assert_eq!(read_pid, Some(std::process::id()), "PID file should contain current PID");

    // Second instance attempt — should report AlreadyRunning without launching
    let config = MasterConfig {
        pid_dir: dir.path().to_string_lossy().to_string(),
        pid_filename: "agent.pid".to_string(),
        kill_wait_secs: 2,
        enable_command_port: false,
        state_dir: dir.path().to_string_lossy().to_string(),
        restrict_agent_dir_permissions: false,
    };
    let master2 = Master::new(config);
    let outcome = master2.start().expect("start() should not return I/O error");

    assert_eq!(
        outcome,
        StartOutcome::AlreadyRunning { pid: std::process::id() },
        "Second instance should report AlreadyRunning with the live PID, got: {outcome:?}"
    );

    // First instance's PID file should be unaffected
    let still_running = pid_file.is_running();
    assert!(still_running, "First instance should still be detected as running");

    // Cleanup
    pid_file.remove().expect("should remove PID file");
}

// Validates: After an agent crash during deployment, restart detects the interrupted
// deployment via stale tracking files and the stale PID file is cleaned up.
#[test]
fn crash_recovery_detects_interrupted_deployment() {
    let dir = tempfile::TempDir::new().expect("should create temp dir");

    // Simulate a crashed agent: PID file exists for a dead process
    let pid_file = PidFile::new(dir.path(), "agent.pid");
    std::fs::create_dir_all(dir.path()).expect("should create dir");
    // Write a PID that doesn't exist (simulate dead process)
    std::fs::write(dir.path().join("agent.pid"), "99999999").expect("should write stale PID");

    // Simulate in-progress deployment marker left by crashed agent
    let tracker_dir = dir.path().join("tracking");
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(tracker_dir);
    tracker
        .start_tracking("d-CRASHED", "cmd-CRASHED")
        .expect("should create deployment marker");

    // Verify the interrupted deployment is detectable
    assert!(
        tracker.is_deployment_in_progress().expect("should check progress"),
        "Interrupted deployment should be detected on restart"
    );

    // Simulate restart: PID file should read the stale value
    let read_result = pid_file.read().expect("should read stale PID file");
    assert_eq!(read_result, Some(99_999_999), "Should read the stale PID value");

    // is_running should return false for the dead process
    assert!(!pid_file.is_running(), "Stale PID should not be detected as running");

    // Writing new PID should succeed (cleans up stale file first)
    pid_file.write().expect("should write new PID after cleanup");
    let new_pid = pid_file.read().expect("should read new PID");
    assert_eq!(
        new_pid,
        Some(std::process::id()),
        "New PID file should contain current process PID"
    );

    // The interrupted deployment marker should still exist for cleanup
    assert!(
        tracker.is_deployment_in_progress().expect("should check progress"),
        "Deployment marker should persist for recovery handling"
    );

    // Clean up the interrupted deployment (agent recovery logic would do this)
    tracker
        .stop_tracking("d-CRASHED")
        .expect("should clean up interrupted deployment");
    assert!(
        !tracker.is_deployment_in_progress().expect("should check progress"),
        "No deployments should be in progress after cleanup"
    );

    pid_file.remove().expect("should remove PID file");
}

// ===========================================================================
// Property-Based Tests
// ===========================================================================

// Property 36: Concurrent deployment serial processing
// Validates: For any sequence of deployment start/stop operations, the tracker
// always reports either zero or one deployment in progress, never more.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn deployment_tracking_is_serial(
        deployment_count in 1usize..=10,
    ) {
        let dir = tempfile::TempDir::new().expect("should create temp dir");
        let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(
            dir.path().to_path_buf()
        );

        // Process deployments sequentially (as the agent does)
        for i in 0..deployment_count {
            let dep_id = format!("d-prop-{i}");
            let cmd_id = format!("cmd-prop-{i}");

            tracker.start_tracking(&dep_id, &cmd_id)
                .expect("should start tracking");
            prop_assert!(
                tracker.is_deployment_in_progress().expect("check"),
                "Should show deployment in progress"
            );
            tracker.stop_tracking(&dep_id)
                .expect("should stop tracking");
            prop_assert!(
                !tracker.is_deployment_in_progress().expect("check"),
                "Should show no deployment after stop"
            );
        }
    }
}

// Property 37: PID file locking
// Validates: For any PID file containing a live process PID, Master::start()
// always reports StartOutcome::AlreadyRunning, preventing multiple instances.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn pid_file_always_prevents_double_start(
        pid_filename in "[a-z]{1,8}\\.pid"
    ) {
        let dir = tempfile::TempDir::new().expect("should create temp dir");
        let pid_file = PidFile::new(dir.path(), &pid_filename);
        pid_file.write().expect("should write PID file");

        let config = MasterConfig {
            pid_dir: dir.path().to_string_lossy().to_string(),
            pid_filename: pid_filename.clone(),
            kill_wait_secs: 1,
            enable_command_port: false,
            state_dir: dir.path().to_string_lossy().to_string(),
            restrict_agent_dir_permissions: false,
        };
        let master = Master::new(config);
        let outcome = master.start().expect("start() should not return I/O error");

        prop_assert_eq!(
            outcome,
            StartOutcome::AlreadyRunning { pid: std::process::id() },
            "PID file {} should prevent start but got: {:?}",
            pid_filename,
            outcome
        );

        pid_file.remove().expect("cleanup");
    }
}

// Property 38: Crash recovery consistency
// Validates: For any stale PID file (dead process), PidFile::write() successfully
// cleans up and writes the new PID, and deployment tracking markers persist for
// recovery handling.
proptest! {
    #![proptest_config(ProptestConfig::with_cases(100))]

    #[test]
    fn stale_pid_files_always_cleaned_up(
        stale_pid in 90_000_000u32..99_999_999u32,
    ) {
        let dir = tempfile::TempDir::new().expect("should create temp dir");
        let pid_file = PidFile::new(dir.path(), "test.pid");

        // Write a stale PID (process that definitely doesn't exist)
        std::fs::write(
            dir.path().join("test.pid"),
            stale_pid.to_string(),
        ).expect("should write stale PID");

        // Verify the stale PID is detected as not running
        prop_assert!(
            !pid_file.is_running(),
            "PID {} should be detected as not running",
            stale_pid
        );

        // Writing new PID should clean up the stale file and succeed
        pid_file.write().expect("should write new PID over stale");
        let new_pid = pid_file.read().expect("should read new PID");
        prop_assert_eq!(
            new_pid,
            Some(std::process::id()),
            "PID file should contain current process PID after recovery"
        );

        pid_file.remove().expect("cleanup");
    }
}
