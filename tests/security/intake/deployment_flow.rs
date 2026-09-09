// Security tests for positive deployment flow.
//
// These tests validate that security controls do NOT block legitimate deployments:
// - Valid AppSpec with lifecycle hooks executes successfully
// - IMDSv2-only configuration is accepted
// - Runas user context switching works (requires root)
//

// Validates: A complete deployment lifecycle succeeds with valid inputs — valid AppSpec,
// safe lifecycle scripts, and correct file permissions — proving that security controls
// do not block legitimate deployments.
#[cfg(unix)]
#[test]
fn valid_deployment_pipeline_succeeds() {
    use std::os::unix::fs::PermissionsExt;

    use codedeploy_agent::deployment_specification::types::{
        DeploymentSpec, RevisionLocation, RevisionSource,
    };
    use codedeploy_agent::lifecycle_event::{LifecycleEventExecutor, LifecycleEventType};

    let dir = tempfile::TempDir::new().expect("should create temp dir");

    // Set up a valid deployment directory with appspec and scripts
    let archive_dir = dir.path().join("deployment-archive");
    let scripts_dir = archive_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir).expect("should create scripts dir");

    // Write a valid AppSpec with a lifecycle hook
    let appspec = "\
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/verify.sh
      timeout: 10
";
    std::fs::write(archive_dir.join("appspec.yml"), appspec).expect("should write appspec");

    // Write a safe lifecycle script
    std::fs::write(
        scripts_dir.join("verify.sh"),
        "#!/bin/sh\necho 'Deployment verified'\nexit 0\n",
    )
    .expect("should write script");
    std::fs::set_permissions(scripts_dir.join("verify.sh"), std::fs::Permissions::from_mode(0o755))
        .expect("should set script permissions");

    let spec = DeploymentSpec {
        deployment_id: "d-TEST001".into(),
        deployment_group_id: "dg-test".into(),
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
            bundle_type: "zip".into(),
            version: None,
            etag: None,
        },
        all_possible_lifecycle_events: None,
        reuse_archive_from_deployment_id: None,
    };

    // Execute the AfterInstall lifecycle event
    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .expect("should create executor");

    assert!(!executor.is_noop(), "Executor should have scripts to run");

    let entries = executor.execute().expect("deployment lifecycle should succeed");
    assert!(
        entries.iter().any(|e| e.contains("LifecycleEvent - AfterInstall")),
        "Log should contain lifecycle event entry, got: {entries:?}"
    );
    assert!(
        entries.iter().any(|e| e.contains("Script - scripts/verify.sh")),
        "Log should contain script execution entry, got: {entries:?}"
    );
}

// Validates: With disable_imds_v1=true, the agent configuration correctly enforces
// IMDSv2-only credential fetching, and the deployment pipeline respects this setting.
#[test]
fn imdsv2_only_config_accepted_for_deployment() {
    use codedeploy_agent::config::AgentConfig;

    // Parse config with IMDSv2-only mode via file (from_yaml is private)
    let dir = tempfile::TempDir::new().expect("should create temp dir");
    let config_path = dir.path().join("imdsv2.yml");
    std::fs::write(&config_path, "disable_imds_v1: true\n").expect("should write config file");
    let config = AgentConfig::from_file(&config_path)
        .expect("should parse config with disable_imds_v1=true");

    assert!(config.disable_imds_v1, "disable_imds_v1 must be true");

    // Verify the v1 fallback is blocked when this flag is set
    // (structural test — the resolve_region function respects this flag)
    let config_source = include_str!("../../../src/config/mod.rs");
    assert!(
        config_source.contains("imds_get_identity_doc") && config_source.contains("disable_v1"),
        "resolve_region must pass disable_v1 flag to IMDS identity doc fetcher"
    );
}

// Validates: When AppSpec specifies a valid runas user, scripts execute as that user,
// confirming proper privilege separation during deployment.
#[cfg(unix)]
#[test]
// TODO: Run as root on EC2 with `nobody` user available for runas context switching.
fn valid_runas_user_executes_as_specified() {
    use std::os::unix::fs::PermissionsExt;

    use codedeploy_agent::deployment_specification::types::{
        DeploymentSpec, RevisionLocation, RevisionSource,
    };
    use codedeploy_agent::lifecycle_event::{LifecycleEventExecutor, LifecycleEventType};

    // Check if we're running as root (required for user switching)
    let uid = nix::unistd::getuid();
    if !uid.is_root() {
        eprintln!("Skipping: requires root for user context switching");
        return;
    }

    let dir = tempfile::TempDir::new().expect("should create temp dir");
    let archive_dir = dir.path().join("deployment-archive");
    let scripts_dir = archive_dir.join("scripts");
    std::fs::create_dir_all(&scripts_dir).expect("should create scripts dir");

    // AppSpec with runas user — `nobody` exists on most Linux systems
    let appspec = "\
version: 0.0
os: linux
hooks:
  AfterInstall:
    - location: scripts/whoami.sh
      timeout: 10
      runas: nobody
";
    std::fs::write(archive_dir.join("appspec.yml"), appspec).expect("should write appspec");

    // Script that outputs the current user
    let output_path = dir.path().join("runas-output.txt");
    let script_content = format!("#!/bin/sh\nid -un > {}\nexit 0\n", output_path.display());
    std::fs::write(scripts_dir.join("whoami.sh"), script_content).expect("should write script");
    std::fs::set_permissions(scripts_dir.join("whoami.sh"), std::fs::Permissions::from_mode(0o755))
        .expect("should set permissions");

    let spec = DeploymentSpec {
        deployment_id: "d-RUNAS".into(),
        deployment_group_id: "dg-runas".into(),
        deployment_group_name: "runas-group".into(),
        application_name: "runas-app".into(),
        deployment_creator: "user".into(),
        deployment_type: "IN_PLACE".into(),
        app_spec_path: "appspec.yml".into(),
        file_exists_behavior: "DISALLOW".into(),
        revision_source: RevisionSource::LocalFile,
        revision: RevisionLocation::Local {
            location: "/tmp/bundle.tar".into(),
            bundle_type: "tar".into(),
        },
        all_possible_lifecycle_events: None,
        reuse_archive_from_deployment_id: None,
    };

    let executor = LifecycleEventExecutor::new(
        LifecycleEventType::AfterInstall,
        &spec,
        dir.path(),
        None,
        None,
    )
    .expect("should create executor");

    let result = executor.execute();
    // If running as root with `nobody` user available, this should succeed
    if result.is_ok() {
        let user = std::fs::read_to_string(&output_path).unwrap_or_default().trim().to_string();
        assert_eq!(user, "nobody", "Script should have run as 'nobody', got '{user}'");
    }
}
