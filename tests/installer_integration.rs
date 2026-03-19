//! Integration tests for the file installer using bundle fixtures.

use aws_codedeploy_agent::application_specification::{AppSpec, FileExistsBehavior};
use aws_codedeploy_agent::installer::Installer;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;
use common::bundle_fixture;

fn copy_dir_recursive(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).unwrap();
    for entry in fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let dest_path = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&entry.path(), &dest_path);
        } else {
            fs::copy(entry.path(), &dest_path).unwrap();
        }
    }
}

// --- Basic file installation ---

#[test]
fn installs_files_from_simple_app_bundle() {
    let archive_dir = TempDir::new().unwrap();
    let instructions_dir = TempDir::new().unwrap();
    let dest_dir = TempDir::new().unwrap();

    // Copy bundle into archive dir
    copy_dir_recursive(&bundle_fixture("simple_app"), archive_dir.path());

    let appspec_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: src/index.html\n    destination: {}",
        dest_dir.path().display()
    );
    let spec = AppSpec::parse(&appspec_yaml).unwrap();

    let installer = Installer::new(
        archive_dir.path().to_path_buf(),
        instructions_dir.path().to_path_buf(),
        FileExistsBehavior::Disallow,
    );

    installer.install("dg-TEST001", &spec).unwrap();

    // Verify file was copied
    assert!(dest_dir.path().join("index.html").exists());
}

#[test]
fn installs_directory_recursively() {
    let archive_dir = TempDir::new().unwrap();
    let instructions_dir = TempDir::new().unwrap();
    let dest_dir = TempDir::new().unwrap();

    copy_dir_recursive(&bundle_fixture("full_hooks_app"), archive_dir.path());

    let appspec_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: app\n    destination: {}",
        dest_dir.path().display()
    );
    let spec = AppSpec::parse(&appspec_yaml).unwrap();

    let installer = Installer::new(
        archive_dir.path().to_path_buf(),
        instructions_dir.path().to_path_buf(),
        FileExistsBehavior::Disallow,
    );

    installer.install("dg-TEST002", &spec).unwrap();

    assert!(dest_dir.path().join("server.py").exists());
}

// --- File exists behavior ---

#[test]
fn overwrite_replaces_existing_file() {
    let archive_dir = TempDir::new().unwrap();
    let instructions_dir = TempDir::new().unwrap();
    let dest_dir = TempDir::new().unwrap();

    copy_dir_recursive(&bundle_fixture("simple_app"), archive_dir.path());

    // Create existing file at destination
    fs::write(dest_dir.path().join("index.html"), "old content").unwrap();

    let appspec_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: src/index.html\n    destination: {}",
        dest_dir.path().display()
    );
    let spec = AppSpec::parse(&appspec_yaml).unwrap();

    let installer = Installer::new(
        archive_dir.path().to_path_buf(),
        instructions_dir.path().to_path_buf(),
        FileExistsBehavior::Overwrite,
    );

    installer.install("dg-TEST003", &spec).unwrap();

    let content = fs::read_to_string(dest_dir.path().join("index.html")).unwrap();
    assert!(content.contains("Deployed with CodeDeploy"));
}

#[test]
fn retain_keeps_existing_file() {
    let archive_dir = TempDir::new().unwrap();
    let instructions_dir = TempDir::new().unwrap();
    let dest_dir = TempDir::new().unwrap();

    copy_dir_recursive(&bundle_fixture("simple_app"), archive_dir.path());

    fs::write(dest_dir.path().join("index.html"), "old content").unwrap();

    let appspec_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: src/index.html\n    destination: {}",
        dest_dir.path().display()
    );
    let spec = AppSpec::parse(&appspec_yaml).unwrap();

    let installer = Installer::new(
        archive_dir.path().to_path_buf(),
        instructions_dir.path().to_path_buf(),
        FileExistsBehavior::Retain,
    );

    installer.install("dg-TEST004", &spec).unwrap();

    let content = fs::read_to_string(dest_dir.path().join("index.html")).unwrap();
    assert_eq!(content, "old content");
}

#[test]
fn disallow_errors_on_existing_file() {
    let archive_dir = TempDir::new().unwrap();
    let instructions_dir = TempDir::new().unwrap();
    let dest_dir = TempDir::new().unwrap();

    copy_dir_recursive(&bundle_fixture("simple_app"), archive_dir.path());

    fs::write(dest_dir.path().join("index.html"), "old content").unwrap();

    let appspec_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: src/index.html\n    destination: {}",
        dest_dir.path().display()
    );
    let spec = AppSpec::parse(&appspec_yaml).unwrap();

    let installer = Installer::new(
        archive_dir.path().to_path_buf(),
        instructions_dir.path().to_path_buf(),
        FileExistsBehavior::Disallow,
    );

    let result = installer.install("dg-TEST005", &spec);
    assert!(result.is_err());
}

// --- Install artifacts ---

#[test]
fn creates_install_json_and_cleanup_file() {
    let archive_dir = TempDir::new().unwrap();
    let instructions_dir = TempDir::new().unwrap();

    let installer = Installer::new(
        archive_dir.path().to_path_buf(),
        instructions_dir.path().to_path_buf(),
        FileExistsBehavior::Disallow,
    );

    let spec = AppSpec::parse("version: 0.0\nos: linux").unwrap();
    installer.install("dg-TEST006", &spec).unwrap();

    assert!(instructions_dir.path().join("dg-TEST006-install.json").exists());
    assert!(instructions_dir.path().join("dg-TEST006-cleanup").exists());
}

// --- Creates missing parent directories ---

#[test]
fn creates_missing_parent_directories() {
    let archive_dir = TempDir::new().unwrap();
    let instructions_dir = TempDir::new().unwrap();
    let dest_dir = TempDir::new().unwrap();

    copy_dir_recursive(&bundle_fixture("simple_app"), archive_dir.path());

    let deep_dest = dest_dir.path().join("a").join("b").join("c");
    let appspec_yaml = format!(
        "version: 0.0\nos: linux\nfiles:\n  - source: src/index.html\n    destination: {}",
        deep_dest.display()
    );
    let spec = AppSpec::parse(&appspec_yaml).unwrap();

    let installer = Installer::new(
        archive_dir.path().to_path_buf(),
        instructions_dir.path().to_path_buf(),
        FileExistsBehavior::Disallow,
    );

    installer.install("dg-TEST007", &spec).unwrap();
    assert!(deep_dest.join("index.html").exists());
}
