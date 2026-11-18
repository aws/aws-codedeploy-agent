//! End-to-end checks that drive the real `codedeploy-agent deploy-local` binary.
//!
//! The binary sets a restrictive 0o027 umask at startup for the agent's own
//! files; the worker resets it to 0o022 before touching customer files.
//! `deploy-local` runs the same install pipeline in-process, so it has to do
//! the same, or the files it deploys get different modes than a service-driven
//! deployment of the same revision would.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn assert_mode(path: &Path, expected: u32) {
    let actual = fs::metadata(path).unwrap().permissions().mode() & 0o7777;
    assert_eq!(actual, expected, "{}: mode {actual:o}, expected {expected:o}", path.display());
}

/// Run `deploy-local` on a directory revision that installs `site/` into a
/// destination whose parents do not exist yet and has an `AfterInstall` hook
/// that creates a directory. Returns `(destination, hook_created_dir)`.
fn deploy(root: &Path) -> (PathBuf, PathBuf) {
    let dest = root.join("var/www/app");
    let hook_dir = root.join("hook-created");

    let bundle = root.join("bundle");
    fs::create_dir_all(bundle.join("site")).unwrap();
    fs::create_dir_all(bundle.join("scripts")).unwrap();
    fs::write(bundle.join("site/index.html"), "<h1>hello</h1>\n").unwrap();
    fs::write(bundle.join("site/secret.conf"), "key=value\n").unwrap();
    set_mode(&bundle.join("site/index.html"), 0o644);
    set_mode(&bundle.join("site/secret.conf"), 0o600);
    let hook = bundle.join("scripts/after_install.sh");
    fs::write(&hook, format!("#!/bin/sh\nmkdir '{}'\n", hook_dir.display())).unwrap();
    set_mode(&hook, 0o755);
    fs::write(
        bundle.join("appspec.yml"),
        format!(
            "version: 0.0\nos: linux\nfiles:\n  - source: site\n    destination: {}\n\
             hooks:\n  AfterInstall:\n    - location: scripts/after_install.sh\n",
            dest.display()
        ),
    )
    .unwrap();

    let config = root.join("agent.yml");
    fs::write(
        &config,
        format!(
            ":log_dir: {}\n:root_dir: {}\n:pid_dir: {}\n",
            root.join("log").display(),
            root.join("deployment-root").display(),
            root.join("pid").display()
        ),
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_codedeploy-agent"))
        .arg("--config-file")
        .arg(&config)
        .args(["deploy-local", "--type", "directory", "--bundle-location"])
        .arg(&bundle)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "deploy-local failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    (dest, hook_dir)
}

#[test]
fn deploy_local_keeps_revision_file_modes() {
    let tmp = TempDir::new().unwrap();
    let (dest, _) = deploy(tmp.path());

    assert_mode(&dest.join("index.html"), 0o644);
    assert_mode(&dest.join("secret.conf"), 0o600);
}

#[test]
fn deploy_local_creates_directories_with_umask_0022() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let (dest, hook_dir) = deploy(root);

    // Directories created by the installer and by a hook script, as a
    // service-driven deployment (worker umask 0o022) creates them.
    assert_mode(&root.join("var"), 0o755);
    assert_mode(&root.join("var/www"), 0o755);
    assert_mode(&dest, 0o755);
    assert_mode(&hook_dir, 0o755);
}
