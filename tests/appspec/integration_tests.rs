// Integration tests using real-world AppSpec examples from AWS documentation
// https://docs.aws.amazon.com/codedeploy/latest/userguide/reference-appspec-file-example.html

use aws_codedeploy_agent::application_specification::AppSpec;

#[test]
fn test_aws_docs_ec2_example() {
    // Official AWS example for EC2/On-Premises deployment
    let yaml = r#"
version: 0.0
os: linux
files:
  - source: Config/config.txt
    destination: /webapps/Config
  - source: source
    destination: /webapps/myApp
hooks:
  BeforeInstall:
    - location: Scripts/UnzipResourceBundle.sh
    - location: Scripts/UnzipDataBundle.sh
  AfterInstall:
    - location: Scripts/RunResourceTests.sh
      timeout: 180
  ApplicationStart:
    - location: Scripts/RunFunctionalTests.sh
      timeout: 3600
  ValidateService:
    - location: Scripts/MonitorService.sh
      timeout: 3600
      runas: codedeployuser
"#;

    let spec = AppSpec::parse(yaml).expect("Failed to parse AWS docs example");

    // Verify version and os
    assert_eq!(spec.version().as_f64(), 0.0);
    assert_eq!(spec.os().as_str(), "linux");

    // Verify files section
    let files: Vec<_> = spec.files().iter().collect();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].source(), "Config/config.txt");
    assert_eq!(files[0].destination(), "/webapps/Config");
    assert_eq!(files[1].source(), "source");
    assert_eq!(files[1].destination(), "/webapps/myApp");

    // Verify hooks
    let before_install = spec.hooks().get("BeforeInstall");
    assert_eq!(before_install.len(), 2);
    assert_eq!(before_install[0].location(), "Scripts/UnzipResourceBundle.sh");
    assert_eq!(before_install[1].location(), "Scripts/UnzipDataBundle.sh");

    let after_install = spec.hooks().get("AfterInstall");
    assert_eq!(after_install.len(), 1);
    assert_eq!(after_install[0].location(), "Scripts/RunResourceTests.sh");
    assert_eq!(after_install[0].timeout(), 180);

    let app_start = spec.hooks().get("ApplicationStart");
    assert_eq!(app_start.len(), 1);
    assert_eq!(app_start[0].location(), "Scripts/RunFunctionalTests.sh");
    assert_eq!(app_start[0].timeout(), 3600);

    let validate = spec.hooks().get("ValidateService");
    assert_eq!(validate.len(), 1);
    assert_eq!(validate[0].location(), "Scripts/MonitorService.sh");
    assert_eq!(validate[0].timeout(), 3600);
    assert_eq!(validate[0].runas(), Some("codedeployuser"));
}

#[test]
fn test_minimal_valid_appspec() {
    let yaml = "version: 0.0\nos: linux\n";
    let spec = AppSpec::parse(yaml).expect("Failed to parse minimal AppSpec");

    assert_eq!(spec.version().as_f64(), 0.0);
    assert_eq!(spec.os().as_str(), "linux");
    assert!(spec.files().iter().count() == 0);
    assert!(spec.permissions().iter().count() == 0);
}

#[test]
fn test_windows_appspec() {
    let yaml = r#"
version: 0.0
os: windows
files:
  - source: Config/config.txt
    destination: c:\webapps\Config
hooks:
  ApplicationStart:
    - location: Scripts/start.bat
      timeout: 300
"#;

    let spec = AppSpec::parse(yaml).expect("Failed to parse Windows AppSpec");
    assert_eq!(spec.os().as_str(), "windows");

    let files: Vec<_> = spec.files().iter().collect();
    assert_eq!(files[0].destination(), "c:\\webapps\\Config");
}

#[test]
fn test_appspec_with_permissions() {
    let yaml = r#"
version: 0.0
os: linux
permissions:
  - object: /var/www
    owner: apache
    group: apache
    mode: 755
    type:
      - directory
  - object: /var/www/index.html
    owner: apache
    group: apache
    mode: 644
    type:
      - file
"#;

    let spec = AppSpec::parse(yaml).expect("Failed to parse AppSpec with permissions");

    let perms: Vec<_> = spec.permissions().iter().collect();
    assert_eq!(perms.len(), 2);
    assert_eq!(perms[0].object(), "/var/www");
    assert_eq!(perms[0].owner(), Some("apache"));
    assert_eq!(perms[0].group(), Some("apache"));
    assert_eq!(perms[1].object(), "/var/www/index.html");
}

#[test]
fn test_appspec_with_file_exists_behavior() {
    let yaml = r#"
version: 0.0
os: linux
file_exists_behavior: OVERWRITE
files:
  - source: app.jar
    destination: /opt/app
"#;

    let spec = AppSpec::parse(yaml).expect("Failed to parse AppSpec with file_exists_behavior");
    assert_eq!(spec.file_exists_behavior().unwrap().as_str(), "OVERWRITE");
}

#[test]
fn test_complex_hooks_with_all_events() {
    let yaml = r#"
version: 0.0
os: linux
hooks:
  ApplicationStop:
    - location: scripts/stop.sh
  DownloadBundle:
    - location: scripts/download.sh
  BeforeInstall:
    - location: scripts/before_install.sh
  AfterInstall:
    - location: scripts/after_install.sh
  ApplicationStart:
    - location: scripts/start.sh
  ValidateService:
    - location: scripts/validate.sh
"#;

    let spec = AppSpec::parse(yaml).expect("Failed to parse complex hooks");

    assert!(!spec.hooks().get("ApplicationStop").is_empty());
    assert!(!spec.hooks().get("DownloadBundle").is_empty());
    assert!(!spec.hooks().get("BeforeInstall").is_empty());
    assert!(!spec.hooks().get("AfterInstall").is_empty());
    assert!(!spec.hooks().get("ApplicationStart").is_empty());
    assert!(!spec.hooks().get("ValidateService").is_empty());
}
