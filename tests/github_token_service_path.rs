//! Live validation of the SERVICE-DRIVEN GitHub private-repo download path.
//!
//! This drives the exact code the CodeDeploy service triggers — it does NOT use
//! the `deploy-local --github-token` CLI path. The flow is:
//!
//!   CodeDeploy-format JSON spec (RevisionType=GitHub + top-level
//!   `GitHubAccessToken`)  →  `DeploymentSpec::new` (the real service parse that
//!   lifts `GitHubAccessToken` into `auth_token`)  →  `DownloadCommand::execute`
//!   →  `GitHubDownloader::authenticated` (`Authorization: token <t>`).
//!
//! Both these tests are `#[ignore]` by default because they need a real private
//! repo + PAT. Provide them via env and run with `--ignored`:
//!
//!   GH_TOKEN=<pat> \
//!   GH_REPO=owner/name \
//!   GH_COMMIT=<sha-or-HEAD> \
//!   cargo test --test github_token_service_path -- --ignored --nocapture
//!
//! The positive test asserts the private bundle downloads + unpacks; the negative
//! test asserts that the SAME spec with NO token fails (proving the repo is truly
//! private and the token is what makes the download work).

use codedeploy_agent::config::AgentConfig;
use codedeploy_agent::deployment_specification::DeploymentSpec;
use codedeploy_agent::host_command::DeploymentArchives;
use codedeploy_agent::host_command::commands::DownloadCommand;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;

/// Split `GH_REPO` (`owner/name`) into (account, repository).
fn repo_parts() -> (String, String) {
    let repo = std::env::var("GH_REPO").expect("set GH_REPO=owner/name");
    let (owner, name) = repo.split_once('/').expect("GH_REPO must be owner/name");
    (owner.to_string(), name.to_string())
}

/// Build a CodeDeploy-format GitHub deployment-spec JSON. When `token` is
/// `Some`, it is placed at the top level as `GitHubAccessToken` exactly as the
/// CodeDeploy service delivers it; when `None`, the spec is anonymous.
fn github_spec_json(
    account: &str,
    repo: &str,
    commit: &str,
    token: Option<&str>,
) -> serde_json::Value {
    let mut spec = json!({
        "DeploymentId": "d-SVCGH001",
        "DeploymentGroupId": "dg-svcgh",
        "DeploymentGroupName": "svc-github-group",
        "ApplicationName": "svc-github-app",
        "Revision": {
            "RevisionType": "GitHub",
            "GitHubRevision": {
                "Account": account,
                "Repository": repo,
                "CommitId": commit,
                "BundleType": "tar"
            }
        }
    });
    if let Some(t) = token {
        spec["GitHubAccessToken"] = json!(t);
    }
    spec
}

fn archives_in(dir: &TempDir) -> Arc<DeploymentArchives> {
    let deploy_root = dir.path().join("deployment-root");
    let instructions = dir.path().join("instructions");
    std::fs::create_dir_all(&deploy_root).unwrap();
    std::fs::create_dir_all(&instructions).unwrap();
    Arc::new(DeploymentArchives::new(deploy_root, instructions, 5))
}

/// POSITIVE: service spec WITH `GitHubAccessToken` downloads + unpacks the
/// private bundle (appspec.yml present in the extracted archive).
#[ignore = "requires GH_TOKEN + GH_REPO + GH_COMMIT for a private repo"]
#[test]
fn service_github_spec_with_token_downloads_private_repo() {
    let token = std::env::var("GH_TOKEN").expect("set GH_TOKEN");
    let commit = std::env::var("GH_COMMIT").unwrap_or_else(|_| "HEAD".to_string());
    let (account, repo) = repo_parts();

    // The REAL service parse: JSON -> DeploymentSpec (lifts GitHubAccessToken).
    let data = github_spec_json(&account, &repo, &commit, Some(&token));
    let spec = DeploymentSpec::new(&data).expect("service parse of GitHub spec failed");

    // Sanity: the parse populated auth_token (anonymous=false) from the spec.
    let dbg = format!("{:?}", spec.revision);
    assert!(dbg.contains("auth_token: Some"), "token not parsed into revision: {dbg}");
    assert!(dbg.contains("anonymous: false"), "expected anonymous=false: {dbg}");

    let work = TempDir::new().unwrap();
    let archives = archives_in(&work);
    let deploy_dir = archives.deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
    std::fs::create_dir_all(&deploy_dir).unwrap();

    let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));
    cmd.execute(&spec).expect("authenticated GitHub download (service path) failed");

    let archive_dir = archives.archive_dir(&spec.deployment_group_id, &spec.deployment_id);
    assert!(
        archive_dir.join("appspec.yml").exists(),
        "expected appspec.yml in extracted private bundle at {}",
        archive_dir.display()
    );
    eprintln!("PASS: service path downloaded + unpacked PRIVATE repo {account}/{repo}@{commit}");
}

/// NEGATIVE: the SAME service spec with NO token must fail (private repo is
/// unreachable anonymously) — proves the token is what makes the positive case
/// work, not public access.
#[ignore = "requires GH_REPO + GH_COMMIT for a private repo"]
#[test]
fn service_github_spec_without_token_fails_on_private_repo() {
    let commit = std::env::var("GH_COMMIT").unwrap_or_else(|_| "HEAD".to_string());
    let (account, repo) = repo_parts();

    let data = github_spec_json(&account, &repo, &commit, None);
    let spec = DeploymentSpec::new(&data).expect("service parse of anonymous GitHub spec failed");

    let dbg = format!("{:?}", spec.revision);
    assert!(dbg.contains("anonymous: true"), "expected anonymous=true: {dbg}");

    let work = TempDir::new().unwrap();
    let archives = archives_in(&work);
    let deploy_dir = archives.deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
    std::fs::create_dir_all(&deploy_dir).unwrap();

    let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));
    let result = cmd.execute(&spec);
    assert!(
        result.is_err(),
        "anonymous download of a PRIVATE repo unexpectedly succeeded — repo may be public"
    );
    eprintln!("PASS: anonymous service-path download of private repo failed as expected");
}
