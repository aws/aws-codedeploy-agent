//! Integration tests for lifecycle hook execution using bundle fixtures.

use codedeploy_agent::deployment_specification::types::{
    DeploymentSpec, RevisionLocation, RevisionSource,
};
use codedeploy_agent::lifecycle_event::{ErrorCode, LifecycleEventExecutor, LifecycleEventType};
use std::path::Path;
use tempfile::TempDir;

mod common;
use common::bundle_fixture;

fn make_spec() -> DeploymentSpec {
    DeploymentSpec {
        deployment_id: "d-A1B2C3D4E".into(),
        deployment_group_id: "dg-F5G6H7I8J".into(),
        deployment_group_name: "test-group".into(),
        application_name: "test-app".into(),
        deployment_creator: "user".into(),
        deployment_type: "IN_PLACE".into(),
        app_spec_path: "appspec.yml".into(),
        file_exists_behavior: "DISALLOW".into(),
        revision_source: RevisionSource::S3,
        revision: RevisionLocation::S3 {
            bucket: "test-bucket".into(),
            key: "test-key".into(),
            bundle_type: "tgz".into(),
            version: Some("v1".into()),
            etag: Some("abc".into()),
        },
        all_possible_lifecycle_events: None,
    }
}

fn setup_deployment_dir(bundle_name: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let archive = dir.path().join("deployment-archive");
    let bundle_src = bundle_fixture(bundle_name);

    copy_dir_recursive(&bundle_src, &archive);
    dir
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let dest_path = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path);
        } else {
            std::fs::copy(entry.path(), &dest_path).unwrap();
            // Preserve executable bit
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let src_mode = std::fs::metadata(entry.path()).unwrap().permissions().mode();
                std::fs::set_permissions(&dest_path, std::fs::Permissions::from_mode(src_mode))
                    .unwrap();
            }
        }
    }
}

// --- Successful execution ---

#[cfg(unix)]
#[test]
fn executes_simple_app_after_install_hook() {
    let dir = setup_deployment_dir("simple_app");
    let spec = make_spec();

    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .unwrap();

    assert!(!executor.is_noop());
    let entries = executor.execute().unwrap();
    assert!(entries.iter().any(|e| e.contains("LifecycleEvent - AfterInstall")));
}

#[cfg(unix)]
#[test]
fn executes_full_hooks_app_after_install() {
    let dir = setup_deployment_dir("full_hooks_app");
    let spec = make_spec();

    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .unwrap();

    assert!(!executor.is_noop());
    let entries = executor.execute().unwrap();
    assert!(entries.iter().any(|e| e.contains("Script - scripts/after_install.sh")));
}

// --- Failing script ---

#[cfg(unix)]
#[test]
fn failing_hook_returns_script_failed_error() {
    let dir = setup_deployment_dir("failing_hook_app");
    let spec = make_spec();

    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .unwrap();

    let result = executor.execute();
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(err.error_code, ErrorCode::ScriptFailed);
    assert!(err.message.contains("exit code 1"));
}

// --- Noop cases ---

#[test]
fn noop_when_no_archive_dir() {
    let dir = TempDir::new().unwrap();
    let spec = make_spec();

    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .unwrap();

    assert!(executor.is_noop());
    let entries = executor.execute().unwrap();
    assert!(entries.is_empty());
}

#[cfg(unix)]
#[test]
fn noop_when_event_has_no_hooks() {
    let dir = setup_deployment_dir("simple_app");
    let spec = make_spec();

    // simple_app only has AfterInstall, not ApplicationStop
    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::ApplicationStop,
        &spec,
        dir.path(),
        None,
        None,
    )
    .unwrap();

    assert!(executor.is_noop());
}

// --- Timeout calculation ---

#[cfg(unix)]
#[test]
fn total_timeout_sums_all_scripts() {
    let dir = setup_deployment_dir("simple_app");
    let spec = make_spec();

    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .unwrap();

    // simple_app AfterInstall has one script with timeout 300
    assert_eq!(executor.total_timeout(), Some(300));
}

#[test]
fn total_timeout_none_for_noop() {
    let dir = TempDir::new().unwrap();
    let spec = make_spec();

    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .unwrap();

    assert_eq!(executor.total_timeout(), None);
}

// --- Environment variables ---

#[cfg(unix)]
#[test]
fn env_vars_available_to_scripts() {
    let dir = TempDir::new().unwrap();
    let archive = dir.path().join("deployment-archive");
    let scripts = archive.join("scripts");
    std::fs::create_dir_all(&scripts).unwrap();

    // Create appspec that runs a script printing env vars
    std::fs::write(
        archive.join("appspec.yml"),
        "version: 0.0\nos: linux\nhooks:\n  AfterInstall:\n    - location: scripts/check_env.sh\n      timeout: 10\n",
    )
    .unwrap();

    let script = scripts.join("check_env.sh");
    std::fs::write(
        &script,
        "#!/bin/bash\necho \"DEPLOYMENT_ID=$DEPLOYMENT_ID\"\necho \"APPLICATION_NAME=$APPLICATION_NAME\"\necho \"LIFECYCLE_EVENT=$LIFECYCLE_EVENT\"\n",
    )
    .unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    let spec = make_spec();
    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .unwrap();

    let entries = executor.execute().unwrap();
    assert!(entries.iter().any(|e| e.contains("DEPLOYMENT_ID=d-A1B2C3D4E")));
    assert!(entries.iter().any(|e| e.contains("APPLICATION_NAME=test-app")));
    assert!(entries.iter().any(|e| e.contains("LIFECYCLE_EVENT=AfterInstall")));
}
