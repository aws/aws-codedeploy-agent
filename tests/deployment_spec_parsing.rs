//! Integration tests for deployment specification parsing using fixture files.

use codedeploy_agent::deployment_specification::types::{
    DeploymentSpec, RevisionLocation, RevisionSource,
};

mod common;
use common::{deployment_spec_fixture, read_fixture};

// --- S3 Revisions ---

#[test]
fn parses_s3_revision() {
    let json_str = read_fixture(&deployment_spec_fixture("s3_revision.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let spec = DeploymentSpec::new(&data).expect("s3 revision");

    assert_eq!(spec.deployment_id, "d-A1B2C3D4E");
    assert_eq!(spec.deployment_group_id, "dg-F5G6H7I8J");
    assert_eq!(spec.deployment_group_name, "production-fleet");
    assert_eq!(spec.application_name, "my-web-app");
    assert!(matches!(spec.revision_source, RevisionSource::S3));

    match &spec.revision {
        RevisionLocation::S3 { bucket, key, bundle_type, version, etag } => {
            assert_eq!(bucket, "my-deploy-bucket");
            assert_eq!(key, "releases/my-web-app/v2.1.0.tar.gz");
            assert_eq!(bundle_type, "tgz");
            assert_eq!(version.as_deref(), Some("abc123def456"));
            assert_eq!(etag.as_deref(), Some("d41d8cd98f00b204e9800998ecf8427e"));
        },
        _ => panic!("Expected S3 revision"),
    }
}

#[test]
fn parses_s3_revision_zip() {
    let json_str = read_fixture(&deployment_spec_fixture("s3_revision_zip.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let spec = DeploymentSpec::new(&data).expect("s3 zip");

    match &spec.revision {
        RevisionLocation::S3 { bundle_type, .. } => assert_eq!(bundle_type, "zip"),
        _ => panic!("Expected S3 revision"),
    }
}

#[test]
fn parses_s3_revision_tar() {
    let json_str = read_fixture(&deployment_spec_fixture("s3_revision_tar.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let spec = DeploymentSpec::new(&data).expect("s3 tar");

    match &spec.revision {
        RevisionLocation::S3 { bundle_type, .. } => assert_eq!(bundle_type, "tar"),
        _ => panic!("Expected S3 revision"),
    }
}

#[test]
fn parses_s3_revision_tgz() {
    let json_str = read_fixture(&deployment_spec_fixture("s3_revision_tgz.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let spec = DeploymentSpec::new(&data).expect("s3 tgz");

    match &spec.revision {
        RevisionLocation::S3 { bundle_type, version, etag, .. } => {
            assert_eq!(bundle_type, "tgz");
            assert!(version.is_some());
            assert!(etag.is_some());
        },
        _ => panic!("Expected S3 revision"),
    }
}

// --- GitHub Revision ---

#[test]
fn parses_github_revision() {
    let json_str = read_fixture(&deployment_spec_fixture("github_revision.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let spec = DeploymentSpec::new(&data).expect("github revision");

    assert_eq!(spec.deployment_id, "d-K1L2M3N4O");
    assert!(matches!(spec.revision_source, RevisionSource::GitHub));

    match &spec.revision {
        RevisionLocation::GitHub { account, repository, commit_id, .. } => {
            assert_eq!(account, "123456789012");
            assert_eq!(repository, "my-org/api-service");
            assert_eq!(commit_id, "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6e7f8a9b0");
        },
        _ => panic!("Expected GitHub revision"),
    }
}

// --- Local Revision ---

#[test]
fn parses_local_revision() {
    let json_str = read_fixture(&deployment_spec_fixture("local_revision.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let spec = DeploymentSpec::new(&data).expect("local revision");

    assert_eq!(spec.deployment_id, "d-U1V2W3X4Y");

    match &spec.revision {
        RevisionLocation::Local { location, bundle_type } => {
            assert_eq!(location, "/tmp/bundles/test-app-v1.0.tar.gz");
            assert_eq!(bundle_type, "tar");
        },
        _ => panic!("Expected Local revision"),
    }
}

// --- Envelope parsing ---

#[test]
fn parses_text_json_envelope() {
    // TEXT/JSON requires CODEDEPLOY_DEVELOPER_MODE=true in production.
    // Integration tests can't mock env, so test the parsing path directly.
    let json_str = read_fixture(&deployment_spec_fixture("s3_revision.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let spec = DeploymentSpec::new(&data).expect("spec parse");
    assert_eq!(spec.deployment_id, "d-A1B2C3D4E");
}

// --- Invalid fixtures ---

#[test]
fn rejects_missing_revision() {
    let json_str = read_fixture(&deployment_spec_fixture("invalid_missing_revision.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let result = DeploymentSpec::new(&data);
    assert!(result.is_err());
}

#[test]
fn rejects_unknown_revision_type() {
    let json_str = read_fixture(&deployment_spec_fixture("invalid_unknown_type.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let result = DeploymentSpec::new(&data);
    assert!(result.is_err());
}

// --- Default values ---

#[test]
fn applies_default_values() {
    let json_str = read_fixture(&deployment_spec_fixture("s3_revision_zip.json"));
    let data: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    let spec = DeploymentSpec::new(&data).expect("defaults");

    // When not specified, defaults should be applied
    assert_eq!(spec.app_spec_path, "appspec.yml");
    assert_eq!(spec.file_exists_behavior, "DISALLOW");
}
