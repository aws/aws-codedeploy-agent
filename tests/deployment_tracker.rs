//! Integration tests for file-based deployment tracking.

use codedeploy_agent::runtime::{DeploymentTracker, FileBasedDeploymentTracker};
use codedeploy_agent::system::SystemFileOperations;
use tempfile::TempDir;

// --- Basic tracking lifecycle ---

#[test]
fn tracks_and_retrieves_active_deployment() {
    let dir = TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    tracker.start_tracking("d-A1B2C3D4E", "cmd-12345").unwrap();

    let active = tracker.get_active_deployment().unwrap().unwrap();
    assert_eq!(active.deployment_id, "d-A1B2C3D4E");
    assert_eq!(active.host_command_identifier, "cmd-12345");
    assert!(active.timestamp > 0);
}

#[test]
fn stop_tracking_removes_deployment() {
    let dir = TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    tracker.start_tracking("d-A1B2C3D4E", "cmd-12345").unwrap();
    tracker.stop_tracking("d-A1B2C3D4E").unwrap();

    assert!(!tracker.is_deployment_in_progress().unwrap());
}

#[test]
fn no_active_deployment_initially() {
    let dir = TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    assert!(tracker.get_active_deployment().unwrap().is_none());
    assert!(!tracker.is_deployment_in_progress().unwrap());
}

// --- Multiple deployments ---

#[test]
fn returns_most_recent_deployment() {
    let dir = TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    tracker.start_tracking("d-OLDER0001", "cmd-1").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    tracker.start_tracking("d-NEWER0001", "cmd-2").unwrap();

    let active = tracker.get_active_deployment().unwrap().unwrap();
    assert_eq!(active.deployment_id, "d-NEWER0001");
}

// --- Clean all ---

#[test]
fn clean_all_removes_tracking_directory() {
    let dir = TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    tracker.start_tracking("d-A1B2C3D4E", "cmd-1").unwrap();
    tracker.start_tracking("d-F5G6H7I8J", "cmd-2").unwrap();

    tracker.clean_all().unwrap();
    assert!(!dir.path().exists());
}

// --- Stale file cleanup ---

#[test]
fn cleans_stale_tracking_files() {
    let dir = TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    tracker.start_tracking("d-STALE0001", "cmd-stale").unwrap();

    // Set file modification time to 25 hours ago (past 24h threshold)
    let old_time = std::time::SystemTime::now() - std::time::Duration::from_secs(25 * 3600);
    filetime::set_file_mtime(
        dir.path().join("d-STALE0001"),
        filetime::FileTime::from_system_time(old_time),
    )
    .unwrap();

    // get_active_deployment triggers stale cleanup
    let active = tracker.get_active_deployment().unwrap();
    assert!(active.is_none());
    assert!(!dir.path().join("d-STALE0001").exists());
}

// --- Edge cases ---

#[test]
fn stop_tracking_nonexistent_is_ok() {
    let dir = TempDir::new().unwrap();
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(dir.path().to_path_buf());

    let result = tracker.stop_tracking("d-NONEXIST1");
    assert!(result.is_ok());
}

#[test]
fn clean_all_nonexistent_dir_is_ok() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().to_path_buf();
    drop(dir);

    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(path);
    assert!(tracker.clean_all().is_ok());
}

#[test]
fn get_active_deployment_nonexistent_dir_returns_none() {
    let dir = TempDir::new().unwrap();
    let nonexistent = dir.path().join("does-not-exist");
    let tracker = FileBasedDeploymentTracker::<SystemFileOperations>::new(nonexistent);

    assert!(tracker.get_active_deployment().unwrap().is_none());
}
