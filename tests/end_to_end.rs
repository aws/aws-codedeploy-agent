//! End-to-end deployment simulation tests.
//!
//! These tests simulate a full deployment lifecycle: parse appspec, install files,
//! execute hooks, and track deployment state. Tests marked `#[ignore]` require
//! unimplemented pipeline orchestration.

use aws_codedeploy_agent::application_specification::{AppSpec, FileExistsBehavior};
use aws_codedeploy_agent::deployment_specification::types::{
    DeploymentSpec, RevisionLocation, RevisionSource,
};
use aws_codedeploy_agent::installer::Installer;
use aws_codedeploy_agent::lifecycle_event::{LifecycleEventExecutor, LifecycleEventType};
use aws_codedeploy_agent::runtime::{DeploymentTracker, FileBasedDeploymentTracker};
use aws_codedeploy_agent::system::SystemFileOperations;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;
use common::bundle_fixture;

fn make_spec(deployment_id: &str, app_name: &str) -> DeploymentSpec {
    DeploymentSpec {
        deployment_id: deployment_id.into(),
        deployment_group_id: "dg-E2ETEST01".into(),
        deployment_group_name: "e2e-test-group".into(),
        application_name: app_name.into(),
        deployment_creator: "user".into(),
        deployment_type: "IN_PLACE".into(),
        app_spec_path: "appspec.yml".into(),
        file_exists_behavior: "DISALLOW".into(),
        revision_source: RevisionSource::S3,
        revision: RevisionLocation::S3 {
            bucket: "e2e-bucket".into(),
            key: "e2e-key".into(),
            bundle_type: "tgz".into(),
            version: None,
            etag: None,
        },
        all_possible_lifecycle_events: None,
    }
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let dest_path = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path);
        } else {
            fs::copy(entry.path(), &dest_path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(entry.path()).unwrap().permissions().mode();
                fs::set_permissions(&dest_path, fs::Permissions::from_mode(mode)).unwrap();
            }
        }
    }
}

/// Simulates a full deployment: track → install files → run hooks → stop tracking.
#[cfg(unix)]
#[test]
fn full_deployment_lifecycle_simple_app() {
    let deployment_root = TempDir::new().unwrap();
    let tracking_dir = TempDir::new().unwrap();
    let dest_dir = TempDir::new().unwrap();

    let spec = make_spec("d-E2E000001", "simple-web-app");

    // 1. Start tracking
    let tracker =
        FileBasedDeploymentTracker::<SystemFileOperations>::new(tracking_dir.path().to_path_buf());
    tracker.start_tracking(&spec.deployment_id, "hci-001").unwrap();
    assert!(tracker.is_deployment_in_progress().unwrap());

    // 2. Set up deployment archive (simulates bundle download)
    let archive_dir = deployment_root.path().join("deployment-archive");
    copy_dir_recursive(&bundle_fixture("simple_app"), &archive_dir);

    // 3. Parse appspec from archive
    let appspec_content = fs::read_to_string(archive_dir.join("appspec.yml")).unwrap();
    let _appspec = AppSpec::parse(&appspec_content).unwrap();

    // 4. Install files
    let instructions_dir = deployment_root.path().join("deployment-instructions");
    fs::create_dir_all(&instructions_dir).unwrap();

    let install_appspec_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: src/index.html\n    destination: {}",
        dest_dir.path().display()
    );
    let install_spec = AppSpec::parse(&install_appspec_yaml).unwrap();
    let installer =
        Installer::new(archive_dir.clone(), instructions_dir.clone(), FileExistsBehavior::Disallow);
    installer.install(&spec.deployment_group_id, &install_spec).unwrap();
    assert!(dest_dir.path().join("index.html").exists());

    // 5. Execute AfterInstall hook
    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        deployment_root.path(),
        None,
        None,
    )
    .unwrap();
    let log_entries = executor.execute().unwrap();
    assert!(log_entries.iter().any(|e| e.contains("AfterInstall")));

    // 6. Stop tracking
    tracker.stop_tracking(&spec.deployment_id).unwrap();
    assert!(!tracker.is_deployment_in_progress().unwrap());
}

/// Tests that a failing hook stops the deployment.
#[cfg(unix)]
#[test]
fn deployment_stops_on_hook_failure() {
    let deployment_root = TempDir::new().unwrap();
    let tracking_dir = TempDir::new().unwrap();

    let spec = make_spec("d-E2EFAIL01", "failing-app");

    let tracker =
        FileBasedDeploymentTracker::<SystemFileOperations>::new(tracking_dir.path().to_path_buf());
    tracker.start_tracking(&spec.deployment_id, "hci-fail").unwrap();

    let archive_dir = deployment_root.path().join("deployment-archive");
    copy_dir_recursive(&bundle_fixture("failing_hook_app"), &archive_dir);

    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        deployment_root.path(),
        None,
        None,
    )
    .unwrap();

    let result = executor.execute();
    assert!(result.is_err());

    // Deployment should still be tracked (not cleaned up on failure)
    assert!(tracker.is_deployment_in_progress().unwrap());

    // Manual cleanup
    tracker.stop_tracking(&spec.deployment_id).unwrap();
}

// --- Tests that were previously ignored, now implemented ---

/// Full deployment pipeline: download (local dir) → install → run all hooks.
#[cfg(unix)]
#[test]
fn pipeline_orchestrates_full_deployment() {
    use aws_codedeploy_agent::host_command::DeploymentArchives;
    use aws_codedeploy_agent::host_command::commands::DownloadCommand;
    use std::sync::Arc;

    let work_dir = TempDir::new().unwrap();
    let deploy_root = work_dir.path().join("deployment-root");
    let instructions = work_dir.path().join("instructions");
    let dest_dir = TempDir::new().unwrap();
    fs::create_dir_all(&deploy_root).unwrap();
    fs::create_dir_all(&instructions).unwrap();

    let archives = Arc::new(DeploymentArchives::new(deploy_root, instructions, 5));

    // Build a spec pointing at the full_hooks_app fixture as a local directory.
    // Override destinations to temp dirs so we don't write to /opt or /etc.
    let bundle_src = bundle_fixture("full_hooks_app");
    let spec = DeploymentSpec {
        deployment_id: "d-PIPELINE01".into(),
        deployment_group_id: "dg-PIPE01".into(),
        deployment_group_name: "pipeline-group".into(),
        application_name: "full-hooks-app".into(),
        deployment_creator: "user".into(),
        deployment_type: "IN_PLACE".into(),
        app_spec_path: "appspec.yml".into(),
        file_exists_behavior: "OVERWRITE".into(),
        revision_source: RevisionSource::LocalDirectory,
        revision: RevisionLocation::Local {
            location: bundle_src.display().to_string(),
            bundle_type: "directory".into(),
        },
        all_possible_lifecycle_events: None,
    };

    // 1. Download bundle (local directory copy)
    let download_cmd = DownloadCommand::new(archives.clone(), None);
    download_cmd.execute(&spec).unwrap();

    // Verify archive was created with appspec
    let archive_dir = archives.archive_dir(&spec.deployment_group_id, &spec.deployment_id);
    assert!(archive_dir.join("appspec.yml").exists());
    assert!(archive_dir.join("scripts/after_install.sh").exists());

    // 2. Install files — rewrite appspec destinations to temp dir
    let install_appspec_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: app/\n    destination: {0}/app\n  - source: config/\n    destination: {0}/config",
        dest_dir.path().display()
    );
    let install_spec =
        aws_codedeploy_agent::application_specification::AppSpec::parse(&install_appspec_yaml)
            .unwrap();
    let installer_obj = Installer::new(
        archive_dir.clone(),
        archives.instructions_dir().to_path_buf(),
        aws_codedeploy_agent::application_specification::FileExistsBehavior::Overwrite,
    );
    installer_obj.install(&spec.deployment_group_id, &install_spec).unwrap();

    assert!(dest_dir.path().join("app/server.py").exists());
    assert!(dest_dir.path().join("config/settings.json").exists());

    // 3. Run lifecycle hooks in order
    let deploy_dir = archives.deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
    for event in [
        LifecycleEventType::BeforeInstall,
        LifecycleEventType::AfterInstall,
        LifecycleEventType::ApplicationStart,
        LifecycleEventType::ValidateService,
    ] {
        let executor = LifecycleEventExecutor::new(event, &spec, &deploy_dir, None, None).unwrap();
        if !executor.is_noop() {
            executor.execute().unwrap();
        }
    }
}

/// Config parsing: load from YAML and verify fields.
#[test]
fn respects_agent_configuration() {
    use aws_codedeploy_agent::config::AgentConfig;

    let config_path = common::fixtures_dir().join("config/default.yml");
    let config = AgentConfig::from_file(&config_path).unwrap();

    // Fixture sets wait_between_runs=1 (default is 30) — proves file was actually loaded
    assert_eq!(config.wait_between_runs, 1);
    assert_eq!(config.max_revisions, 5);
    assert_eq!(config.http_read_timeout, 80);
}

/// Deployment cleanup: old archives pruned beyond max_revisions.
#[test]
fn cleans_up_old_deployments_beyond_max_revisions() {
    use aws_codedeploy_agent::host_command::DeploymentArchives;
    use std::sync::Arc;

    let work_dir = TempDir::new().unwrap();
    let deploy_root = work_dir.path().join("deployment-root");
    let instructions = work_dir.path().join("instructions");
    fs::create_dir_all(&deploy_root).unwrap();
    fs::create_dir_all(&instructions).unwrap();

    // max_revisions = 2
    let archives = Arc::new(DeploymentArchives::new(deploy_root.clone(), instructions, 2));
    let group = "dg-CLEANUP01";

    // Create 4 deployments
    for i in 1..=4 {
        let id = format!("d-{i:03}");
        let dir = archives.deployment_root_dir(group, &id);
        fs::create_dir_all(&dir).unwrap();
        // Small delay so mtimes differ
        std::thread::sleep(std::time::Duration::from_millis(20));
    }

    // Cleanup with d-004 as current
    let current = archives.deployment_root_dir(group, "d-004");
    archives.cleanup_old_archives(group, &current).unwrap();

    // Should keep current (d-004) + max_revisions (2) = at most 3
    let remaining: Vec<_> = fs::read_dir(deploy_root.join(group))
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .collect();

    assert!(
        remaining.len() >= 2 && remaining.len() <= 3,
        "Expected 2-3 dirs (max_revisions=2 + current), got {}",
        remaining.len()
    );
    // Current deployment must survive
    assert!(current.exists());
}

/// Full pipeline from a tar archive: download → unpack → install → hooks.
/// Exercises the same code path as S3 (S3 produces a file on disk, then unpack runs).
#[cfg(unix)]
#[test]
fn downloads_tar_bundle_and_deploys() {
    use aws_codedeploy_agent::host_command::DeploymentArchives;
    use aws_codedeploy_agent::host_command::commands::DownloadCommand;
    use std::sync::Arc;

    let work_dir = TempDir::new().unwrap();
    let deploy_root = work_dir.path().join("deployment-root");
    let instructions = work_dir.path().join("instructions");
    let dest_dir = TempDir::new().unwrap();
    fs::create_dir_all(&deploy_root).unwrap();
    fs::create_dir_all(&instructions).unwrap();

    // Create a tar from the simple_app fixture
    let tar_path = work_dir.path().join("bundle.tar");
    let bundle_src = bundle_fixture("simple_app");
    let status = std::process::Command::new("tar")
        .args([
            "-cf",
            &tar_path.display().to_string(),
            "-C",
            &bundle_src.display().to_string(),
            ".",
        ])
        .status()
        .unwrap();
    assert!(status.success(), "tar creation failed");

    let archives = Arc::new(DeploymentArchives::new(deploy_root, instructions, 5));

    let spec = DeploymentSpec {
        deployment_id: "d-TAR00001".into(),
        deployment_group_id: "dg-TAR01".into(),
        deployment_group_name: "tar-group".into(),
        application_name: "simple-web-app".into(),
        deployment_creator: "user".into(),
        deployment_type: "IN_PLACE".into(),
        app_spec_path: "appspec.yml".into(),
        file_exists_behavior: "OVERWRITE".into(),
        revision_source: RevisionSource::LocalFile,
        revision: RevisionLocation::Local {
            location: tar_path.display().to_string(),
            bundle_type: "tar".into(),
        },
        all_possible_lifecycle_events: None,
    };

    // 1. Download + unpack
    // Pre-create deploy_dir: LocalFile downloads write into an existing deployment root,
    // unlike LocalDirectory which creates it during copy.
    let deploy_dir = archives.deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
    fs::create_dir_all(&deploy_dir).unwrap();
    let download_cmd = DownloadCommand::new(archives.clone(), None);
    download_cmd.execute(&spec).unwrap();

    let archive_dir = archives.archive_dir(&spec.deployment_group_id, &spec.deployment_id);
    assert!(archive_dir.join("appspec.yml").exists());
    assert!(archive_dir.join("src/index.html").exists());
    assert!(archive_dir.join("scripts/restart_server.sh").exists());

    // 2. Install files to temp dest
    let install_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: src/index.html\n    destination: {}",
        dest_dir.path().display()
    );
    let install_spec =
        aws_codedeploy_agent::application_specification::AppSpec::parse(&install_yaml).unwrap();
    let installer_obj = Installer::new(
        archive_dir.clone(),
        archives.instructions_dir().to_path_buf(),
        aws_codedeploy_agent::application_specification::FileExistsBehavior::Overwrite,
    );
    installer_obj.install(&spec.deployment_group_id, &install_spec).unwrap();
    assert!(dest_dir.path().join("index.html").exists());

    // 3. Run AfterInstall hook
    let deploy_dir = archives.deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        &deploy_dir,
        None,
        None,
    )
    .unwrap();
    let entries = executor.execute().unwrap();
    assert!(entries.iter().any(|e| e.contains("AfterInstall")));
}

#[ignore = "Requires two sequential deployments with rollback state"]
#[test]
fn rollback_uses_last_successful_deployment() {
    // Needs last_successful_install tracking across two deployments
}
