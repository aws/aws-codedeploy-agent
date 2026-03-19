//! Integration tests for AppSpec YAML parsing using fixture files.
//!
//! Tests the application_specification module against real-world AppSpec examples.

use aws_codedeploy_agent::application_specification::{AppSpec, ParseError};

mod common;
use common::{appspec_fixture, read_fixture};

// --- Valid EC2/Linux fixtures ---

#[test]
fn parses_ec2_linux_simple() {
    let yaml = read_fixture(&appspec_fixture("ec2_linux_simple.yml"));
    let spec = AppSpec::parse(&yaml).expect("simple linux appspec");

    assert_eq!(spec.os().as_str(), "linux");
    assert_eq!(spec.files().iter().count(), 1);

    let file = spec.files().iter().next().unwrap();
    assert_eq!(file.source(), "/");
    assert_eq!(file.destination(), "/var/www/html");

    let hooks = spec.hooks().get("AfterInstall");
    assert_eq!(hooks.len(), 1);
    assert_eq!(hooks[0].location(), "scripts/restart_server.sh");
    assert_eq!(hooks[0].timeout(), 300);
}

#[test]
fn parses_ec2_linux_full() {
    let yaml = read_fixture(&appspec_fixture("ec2_linux_full.yml"));
    let spec = AppSpec::parse(&yaml).expect("full linux appspec");

    assert_eq!(spec.files().iter().count(), 2);

    // Verify all hooks present
    assert!(!spec.hooks().get("ApplicationStop").is_empty());
    assert!(!spec.hooks().get("BeforeInstall").is_empty());
    assert!(!spec.hooks().get("AfterInstall").is_empty());
    assert!(!spec.hooks().get("ApplicationStart").is_empty());
    assert!(!spec.hooks().get("ValidateService").is_empty());

    // Verify runas
    let stop = &spec.hooks().get("ApplicationStop")[0];
    assert_eq!(stop.runas(), Some("webapp"));

    // Verify multiple scripts in BeforeInstall
    assert_eq!(spec.hooks().get("BeforeInstall").len(), 2);

    // Verify permissions
    assert_eq!(spec.permissions().iter().count(), 2);
    let perm = spec.permissions().iter().next().unwrap();
    assert_eq!(perm.object(), "/webapps/myApp");
    assert_eq!(perm.owner(), Some("webapp"));
    assert_eq!(perm.group(), Some("webapp"));
}

#[test]
fn parses_ec2_windows() {
    let yaml = read_fixture(&appspec_fixture("ec2_windows.yml"));
    let spec = AppSpec::parse(&yaml).expect("windows appspec");

    assert_eq!(spec.os().as_str(), "windows");
    assert!(!spec.hooks().get("ApplicationStop").is_empty());
    assert!(!spec.hooks().get("ApplicationStart").is_empty());
}

#[test]
fn parses_ec2_files_only() {
    let yaml = read_fixture(&appspec_fixture("ec2_files_only.yml"));
    let spec = AppSpec::parse(&yaml).expect("files only appspec");

    assert_eq!(spec.files().iter().count(), 2);
    assert!(spec.hooks().get("BeforeInstall").is_empty());
}

#[test]
fn parses_ec2_hooks_only() {
    let yaml = read_fixture(&appspec_fixture("ec2_hooks_only.yml"));
    let spec = AppSpec::parse(&yaml).expect("hooks only appspec");

    assert_eq!(spec.files().iter().count(), 0);
    assert!(!spec.hooks().get("BeforeInstall").is_empty());
    assert!(!spec.hooks().get("ValidateService").is_empty());
}

#[test]
fn parses_ec2_permissions_full() {
    let yaml = read_fixture(&appspec_fixture("ec2_permissions_full.yml"));
    let spec = AppSpec::parse(&yaml).expect("full permissions appspec");

    let perms: Vec<_> = spec.permissions().iter().collect();
    assert_eq!(perms.len(), 2);

    // Directory permission with ACLs and SELinux context
    let dir_perm = &perms[0];
    assert_eq!(dir_perm.object(), "/opt/secure-app");
    assert_eq!(dir_perm.owner(), Some("deploy"));
    assert!(dir_perm.acls().is_some());
    assert!(dir_perm.context().is_some());

    // File permission with ACLs
    let file_perm = &perms[1];
    assert_eq!(file_perm.object(), "/opt/secure-app/config/secrets.conf");
    assert!(file_perm.acls().is_some());
}

#[test]
fn parses_ec2_multiple_scripts_per_hook() {
    let yaml = read_fixture(&appspec_fixture("ec2_multiple_scripts_per_hook.yml"));
    let spec = AppSpec::parse(&yaml).expect("multiple scripts appspec");

    let before = spec.hooks().get("BeforeInstall");
    assert_eq!(before.len(), 3);
    assert_eq!(before[0].location(), "scripts/step1_backup.sh");
    assert_eq!(before[1].location(), "scripts/step2_cleanup.sh");
    assert_eq!(before[2].location(), "scripts/step3_prepare.sh");

    let after = spec.hooks().get("AfterInstall");
    assert_eq!(after.len(), 2);
}

// --- File exists behavior ---

#[test]
fn parses_file_exists_overwrite() {
    let yaml = read_fixture(&appspec_fixture("ec2_file_exists_overwrite.yml"));
    let spec = AppSpec::parse(&yaml).expect("overwrite appspec");
    assert_eq!(spec.file_exists_behavior().unwrap().as_str(), "OVERWRITE");
}

#[test]
fn parses_file_exists_retain() {
    let yaml = read_fixture(&appspec_fixture("ec2_file_exists_retain.yml"));
    let spec = AppSpec::parse(&yaml).expect("retain appspec");
    assert_eq!(spec.file_exists_behavior().unwrap().as_str(), "RETAIN");
}

#[test]
fn parses_file_exists_disallow() {
    let yaml = read_fixture(&appspec_fixture("ec2_file_exists_disallow.yml"));
    let spec = AppSpec::parse(&yaml).expect("disallow appspec");
    assert_eq!(spec.file_exists_behavior().unwrap().as_str(), "DISALLOW");
}

// --- Invalid fixtures ---

#[test]
fn rejects_empty_file() {
    let yaml = read_fixture(&appspec_fixture("invalid_empty.yml"));
    let result = AppSpec::parse(&yaml);
    assert!(result.is_err());
}

#[test]
fn rejects_bad_yaml() {
    let yaml = read_fixture(&appspec_fixture("invalid_bad_yaml.yml"));
    let result = AppSpec::parse(&yaml);
    assert!(matches!(result, Err(ParseError::YamlError(_))));
}

#[test]
fn rejects_unknown_os() {
    let yaml = read_fixture(&appspec_fixture("invalid_unknown_os.yml"));
    let result = AppSpec::parse(&yaml);
    assert!(matches!(result, Err(ParseError::UnsupportedOs(_))));
}

#[test]
fn rejects_empty_hook_location() {
    let yaml = read_fixture(&appspec_fixture("invalid_unknown_hook.yml"));
    let result = AppSpec::parse(&yaml);
    assert!(matches!(result, Err(ParseError::EmptyScriptLocation)));
}

#[test]
fn rejects_missing_version() {
    let yaml = read_fixture(&appspec_fixture("invalid_missing_version.yml"));
    let result = AppSpec::parse(&yaml);
    // Missing version should cause a YAML parse error or InvalidVersion
    assert!(result.is_err());
}

#[test]
fn rejects_no_os() {
    let yaml = read_fixture(&appspec_fixture("invalid_no_os.yml"));
    let result = AppSpec::parse(&yaml);
    assert!(result.is_err());
}

// --- From file ---

#[test]
fn parses_from_file_path() {
    let path = appspec_fixture("ec2_linux_simple.yml");
    let spec = AppSpec::from_file(&path).expect("from_file");
    assert_eq!(spec.os().as_str(), "linux");
}

#[test]
fn from_file_nonexistent_returns_error() {
    let result = AppSpec::from_file("/nonexistent/appspec.yml");
    assert!(result.is_err());
}
