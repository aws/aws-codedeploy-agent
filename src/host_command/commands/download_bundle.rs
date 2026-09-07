//! @risk medium
//!
//! `DownloadBundle` command.
//!
//! Cleans up old archives, downloads the bundle from the appropriate source
//! (S3, GitHub, local file/directory), unpacks if not a directory, and records
//! the most recent install.

use crate::aws_clients::S3Client;
use crate::config::AgentConfig;
use crate::deployment_specification::types::{DeploymentSpec, RevisionLocation, RevisionSource};
use crate::host_command::DeploymentArchives;
use crate::host_command::bundle_downloader::{
    BundleDownloader, BundleFormat, GitHubDownloader, LocalDirectoryDownloader,
    LocalFileDownloader, S3Downloader,
};
use crate::host_command::bundle_unpacker;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

#[derive(Debug)]
pub struct DownloadCommand {
    archives: Arc<DeploymentArchives>,
    s3_client: Option<S3Client>,
    config: Arc<AgentConfig>,
}

impl DownloadCommand {
    #[must_use]
    pub fn new(
        archives: Arc<DeploymentArchives>,
        s3_client: Option<S3Client>,
        config: Arc<AgentConfig>,
    ) -> Self {
        Self { archives, s3_client, config }
    }

    /// Execute the `DownloadBundle` command.
    ///
    /// # Errors
    /// Returns an error if download, unpack, or archive management fails.
    pub fn execute(&self, spec: &DeploymentSpec) -> io::Result<()> {
        let deploy_dir = self
            .archives
            .deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
        let archive_dir = self.archives.archive_dir(&spec.deployment_group_id, &spec.deployment_id);
        let bundle_path = self
            .archives
            .artifact_bundle_path(&spec.deployment_group_id, &spec.deployment_id);

        self.archives.cleanup_old_archives(&spec.deployment_group_id, &deploy_dir)?;

        debug!("Executing DownloadBundle command");

        let actual_etag = self.download(spec, &bundle_path, &archive_dir)?;

        self.settle_bundle_mode(&bundle_path)?;

        // Persist the actual S3 ETag observed on download so the lifecycle-event
        // executor can expose it to hooks as BUNDLE_ETAG. The deployment spec
        // often carries a null ETag (the service does not always populate it),
        // so reading it back from the spec alone leaves BUNDLE_ETAG unset; the
        // agent saw the real value here, so record it next to the deployment.
        if let Some(etag) = actual_etag.as_deref() {
            let etag_path = deploy_dir.join(crate::host_command::BUNDLE_ETAG_FILE);
            if let Err(e) = crate::system::write_file_secure(
                &etag_path,
                etag.as_bytes(),
                crate::system::agent_file_mode(
                    self.config.hardening.restrict_agent_dir_permissions,
                ),
            ) {
                // Non-fatal: BUNDLE_ETAG is best-effort metadata, not required
                // for a correct deployment.
                warn!(path = %etag_path.display(), "Failed to persist bundle ETag: {e}");
            }
        }

        info!(
            revision_source = ?spec.revision_source,
            deployment_id = %spec.deployment_id,
            "Bundle downloaded"
        );

        if !matches!(spec.revision_source, RevisionSource::LocalDirectory) {
            if archive_dir.exists() {
                fs::remove_dir_all(&archive_dir)?;
            }
            // Size check runs pre-extraction (inspects headers only, no disk writes).
            if let Some(max_size) = self.config.archive_max_extraction_size
                && let Err(e) = bundle_unpacker::check_extraction_size(
                    &bundle_path,
                    &Self::bundle_type(spec),
                    max_size,
                )
            {
                if let Err(rm_err) = fs::remove_file(&bundle_path) {
                    tracing::warn!(
                        path = %bundle_path.display(),
                        error = %rm_err,
                        "Failed to remove rejected bundle"
                    );
                }
                return Err(e);
            }

            if self.config.hardening.reject_path_traversal_in_bundle
                && let Err(e) =
                    bundle_unpacker::check_path_traversal(&bundle_path, &Self::bundle_type(spec))
            {
                // rejected bundle fails; not reproducible in CI.
                if let Err(rm_err) = fs::remove_file(&bundle_path) {
                    tracing::warn!(
                        path = %bundle_path.display(),
                        error = %rm_err,
                        "Failed to remove rejected bundle"
                    );
                }
                return Err(e);
            }

            bundle_unpacker::unpack(
                &bundle_path,
                &archive_dir,
                &Self::bundle_type(spec),
                self.config.hardening.restrict_agent_dir_permissions,
                self.config.hardening.ignore_ownership_in_bundle,
            )?;
        }

        if self.config.hardening.reject_symlinks_in_bundle {
            bundle_unpacker::reject_bundle_symlinks(&archive_dir)?;
        }

        if self.config.hardening.reject_path_traversal_in_bundle {
            bundle_unpacker::reject_bundle_path_traversal(&archive_dir)?;
        }

        if self.config.hardening.reject_unsafe_permissions_in_bundle {
            bundle_unpacker::reject_bundle_unsafe_permissions(&archive_dir)?;
        }

        let instructions_dir = self.archives.instructions_dir();
        crate::system::create_deployment_dir(
            instructions_dir,
            0o700,
            self.config.hardening.restrict_agent_dir_permissions,
        )?;
        debug!("Instructions directory created at {}", instructions_dir.display());

        // The appspec is also validated during Install; we check earlier at
        // download time to fail fast with a clear message rather than letting
        // Install discover it. NOTE: cleanup_old_archives has already run at
        // this point (destructive-then-validate ordering), so a missing appspec
        // here means the old archive may already be gone.
        let appspec_path = archive_dir.join(&spec.app_spec_path);
        if !appspec_path.exists() {
            return Err(io::Error::other(format!(
                "The deployment failed because the specified file does not exist at the expected \
                 location: {}. Verify that your AppSpec file is named correctly and that it is in \
                 the root directory of the revision's source code.",
                appspec_path.display()
            )));
        }

        self.archives.update_most_recent(&spec.deployment_group_id, &deploy_dir)?;

        Ok(())
    }

    /// Settle the downloaded bundle to the `restrict_agent_dir_permissions`
    /// policy mode. The downloaders create it 0600 (safe while streaming); this
    /// is the single chokepoint for S3/GitHub/local-file sources. The default
    /// (unhardened) mode is 0644.
    ///
    /// Skips symlinks: the `LocalFile` source symlinks `bundle_path` at the user's
    /// ORIGINAL file (`local_file.rs`), and `set_permissions` (chmod) follows
    /// the link — so chmod'ing here would silently rewrite the mode of the
    /// customer's own file outside the agent tree. The symlinked bundle is not
    /// agent-owned and needs no mode settling; only real downloaded files
    /// (S3/GitHub) do. `is_symlink` uses `symlink_metadata` and does not follow
    /// the link.
    #[cfg_attr(not(unix), allow(clippy::unused_self, unused_variables))]
    fn settle_bundle_mode(&self, bundle_path: &Path) -> io::Result<()> {
        #[cfg(unix)]
        if bundle_path.exists() && !bundle_path.is_symlink() {
            use std::os::unix::fs::PermissionsExt;
            let mode = crate::system::agent_file_mode(
                self.config.hardening.restrict_agent_dir_permissions,
            );
            fs::set_permissions(bundle_path, fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    }

    /// Download the revision bundle. Returns the S3 object's actual `ETag` for an
    /// S3 revision (so the caller can expose it to hooks as `BUNDLE_ETAG`), or
    /// `None` for non-S3 sources.
    fn download(
        &self,
        spec: &DeploymentSpec,
        bundle_path: &Path,
        archive_dir: &Path,
    ) -> io::Result<Option<String>> {
        match &spec.revision {
            RevisionLocation::S3 { bucket, key, version, etag, .. } => {
                let client = self.s3_client.as_ref().ok_or_else(|| {
                    io::Error::other("S3 client not configured for S3 revision source")
                })?;
                S3Downloader::new(
                    client,
                    bucket.clone(),
                    key.clone(),
                    version.clone(),
                    etag.clone(),
                    bundle_path.to_path_buf(),
                )
                .download_returning_etag()
            },
            RevisionLocation::GitHub { .. } => {
                let downloader = Self::build_github_downloader(
                    spec,
                    bundle_path,
                    self.config.proxy_uri.clone(),
                )?;
                downloader.download().map(|()| None)
            },
            RevisionLocation::Local { location, bundle_type: _ } => {
                if spec.revision_source == RevisionSource::LocalDirectory {
                    LocalDirectoryDownloader::new(
                        PathBuf::from(location),
                        archive_dir.to_path_buf(),
                        self.config.hardening.restrict_agent_dir_permissions,
                    )
                    .download()
                    .map(|()| None)
                } else {
                    LocalFileDownloader::new(PathBuf::from(location), bundle_path.to_path_buf())
                        .download()
                        .map(|()| None)
                }
            },
        }
    }

    fn build_github_downloader(
        spec: &DeploymentSpec,
        bundle_path: &Path,
        proxy_uri: Option<String>,
    ) -> io::Result<GitHubDownloader> {
        let RevisionLocation::GitHub {
            account,
            repository,
            commit_id,
            anonymous,
            auth_token,
            bundle_type,
        } = &spec.revision
        else {
            return Err(io::Error::other("Not a GitHub revision"));
        };

        let format = BundleFormat::from_bundle_type(bundle_type.as_deref())?;
        let dest = bundle_path.to_path_buf();
        if *anonymous {
            Ok(GitHubDownloader::anonymous(
                account.clone(),
                repository.clone(),
                commit_id.clone(),
                format,
                dest,
                proxy_uri,
            ))
        } else {
            Ok(GitHubDownloader::authenticated(
                account.clone(),
                repository.clone(),
                commit_id.clone(),
                auth_token.clone().unwrap_or_default(),
                format,
                dest,
                proxy_uri,
            ))
        }
    }

    fn bundle_type(spec: &DeploymentSpec) -> String {
        match &spec.revision {
            RevisionLocation::S3 { bundle_type, .. }
            | RevisionLocation::Local { bundle_type, .. } => bundle_type.clone(),
            RevisionLocation::GitHub { bundle_type, .. } => {
                bundle_type.clone().unwrap_or_else(|| "tar".to_string())
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{
        DeploymentSpec, RevisionLocation, RevisionSource,
    };
    use tempfile::TempDir;

    fn test_archives(dir: &TempDir) -> Arc<DeploymentArchives> {
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&instructions).unwrap();
        Arc::new(DeploymentArchives::new(root, instructions, 5))
    }

    fn local_file_spec(location: &str) -> DeploymentSpec {
        DeploymentSpec {
            deployment_id: "d-123".into(),
            deployment_group_id: "dg-1".into(),
            deployment_group_name: "my-group".into(),
            application_name: "my-app".into(),
            deployment_creator: "user".into(),
            deployment_type: "IN_PLACE".into(),
            app_spec_path: "appspec.yml".into(),
            file_exists_behavior: "DISALLOW".into(),
            revision_source: RevisionSource::LocalFile,
            revision: RevisionLocation::Local {
                location: location.into(),
                bundle_type: "tar".into(),
            },
            all_possible_lifecycle_events: None,
        }
    }

    fn local_dir_spec(location: &str) -> DeploymentSpec {
        DeploymentSpec {
            revision_source: RevisionSource::LocalDirectory,
            revision: RevisionLocation::Local {
                location: location.into(),
                bundle_type: "directory".into(),
            },
            ..local_file_spec(location)
        }
    }

    fn s3_spec() -> DeploymentSpec {
        DeploymentSpec {
            revision_source: RevisionSource::S3,
            revision: RevisionLocation::S3 {
                bucket: "my-bucket".into(),
                key: "my-key".into(),
                bundle_type: "tar".into(),
                version: None,
                etag: None,
            },
            ..local_file_spec("")
        }
    }

    #[test]
    fn bundle_type_s3() {
        let spec = s3_spec();
        assert_eq!(DownloadCommand::bundle_type(&spec), "tar");
    }

    #[test]
    fn bundle_type_local() {
        let spec = local_file_spec("/tmp/bundle.tgz");
        assert_eq!(DownloadCommand::bundle_type(&spec), "tar");
    }

    #[test]
    fn bundle_type_github_default() {
        let spec = DeploymentSpec {
            revision_source: RevisionSource::GitHub,
            revision: RevisionLocation::GitHub {
                account: "acme".into(),
                repository: "app".into(),
                commit_id: "abc".into(),
                anonymous: true,
                auth_token: None,
                bundle_type: None,
            },
            ..local_file_spec("")
        };
        assert_eq!(DownloadCommand::bundle_type(&spec), "tar");
    }

    #[test]
    fn s3_without_client_errors() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = DownloadCommand::new(archives, None, Arc::new(AgentConfig::default()));

        let spec = s3_spec();
        let deploy_dir = cmd.archives.deployment_root_dir("dg-1", "d-123");
        fs::create_dir_all(&deploy_dir).unwrap();

        let err = cmd.execute(&spec).unwrap_err();
        assert!(err.to_string().contains("S3 client not configured"));
    }

    #[cfg(unix)]
    #[test]
    fn local_file_creates_symlink_and_unpacks() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));

        // Create a source tar with an appspec
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("appspec.yml"), "version: 0.0\nos: linux").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        std::process::Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src_dir.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        // Give the source file a distinctive mode so we can prove the
        // permission-settle step does NOT follow the bundle symlink and chmod
        // the user's original file (the bundle is symlinked at it).
        fs::set_permissions(&tar_path, fs::Permissions::from_mode(0o640)).unwrap();

        let spec = local_file_spec(&tar_path.display().to_string());

        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        fs::create_dir_all(&deploy_dir).unwrap();

        cmd.execute(&spec).unwrap();

        // Bundle symlinked
        let bundle = archives.artifact_bundle_path("dg-1", "d-123");
        assert!(bundle.is_symlink());

        // The user's original source file keeps its mode — the settle step
        // must skip the symlinked bundle rather than chmod through it (0644
        // is the default policy mode that would have been applied).
        let src_mode = fs::metadata(&tar_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            src_mode, 0o640,
            "LocalFile source must not be chmod'd through the bundle symlink, got {src_mode:#o}"
        );

        // Archive unpacked
        let archive = archives.archive_dir("dg-1", "d-123");
        assert!(archive.join("appspec.yml").exists());

        // Most recent updated
        assert!(archives.most_recent_dir("dg-1").is_some());

        // Instructions dir created
        assert!(archives.instructions_dir().exists());
    }

    #[test]
    fn local_directory_copies_without_unpack() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));

        // Create source directory
        let src_dir = dir.path().join("my-app");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("appspec.yml"), "version: 0.0").unwrap();
        fs::write(src_dir.join("script.sh"), "#!/bin/sh").unwrap();

        let spec = local_dir_spec(&src_dir.display().to_string());

        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        fs::create_dir_all(&deploy_dir).unwrap();

        cmd.execute(&spec).unwrap();

        // Archive dir has copied content (no unpack step)
        let archive = archives.archive_dir("dg-1", "d-123");
        assert!(archive.join("appspec.yml").exists());
        assert!(archive.join("script.sh").exists());

        // Most recent updated
        assert!(archives.most_recent_dir("dg-1").is_some());
    }

    #[cfg(unix)]
    #[test]
    fn local_file_removes_existing_archive_dir_before_unpack() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));

        // Create a source tar
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("appspec.yml"), "version: 0.0\nos: linux").unwrap();

        let tar_path = dir.path().join("bundle.tar");
        std::process::Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src_dir.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let spec = local_file_spec(&tar_path.display().to_string());
        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        fs::create_dir_all(&deploy_dir).unwrap();

        // Pre-create archive dir with stale content — should be removed before unpack
        let archive_dir = archives.archive_dir("dg-1", "d-123");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("stale.txt"), "old").unwrap();

        cmd.execute(&spec).unwrap();

        // Stale file should be gone, fresh unpack should be present
        assert!(!archive_dir.join("stale.txt").exists());
        assert!(archive_dir.join("appspec.yml").exists());
    }

    #[test]
    fn bundle_type_github_with_zip() {
        let spec = DeploymentSpec {
            revision_source: RevisionSource::GitHub,
            revision: RevisionLocation::GitHub {
                account: "acme".into(),
                repository: "app".into(),
                commit_id: "abc".into(),
                anonymous: true,
                auth_token: None,
                bundle_type: Some("zip".into()),
            },
            ..local_file_spec("")
        };
        assert_eq!(DownloadCommand::bundle_type(&spec), "zip");
    }

    #[test]
    fn build_downloader_s3_without_client() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));

        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        fs::create_dir_all(&deploy_dir).unwrap();

        let result = cmd.execute(&s3_spec());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("S3 client not configured"));
    }

    #[test]
    fn build_github_downloader_anonymous() {
        let spec = DeploymentSpec {
            revision_source: RevisionSource::GitHub,
            revision: RevisionLocation::GitHub {
                account: "acme".into(),
                repository: "app".into(),
                commit_id: "abc".into(),
                anonymous: true,
                auth_token: None,
                bundle_type: Some("tar".into()),
            },
            ..local_file_spec("")
        };
        let result = DownloadCommand::build_github_downloader(&spec, Path::new("/tmp/out"), None);
        assert!(result.is_ok());
    }

    #[test]
    fn build_github_downloader_authenticated() {
        let spec = DeploymentSpec {
            revision_source: RevisionSource::GitHub,
            revision: RevisionLocation::GitHub {
                account: "acme".into(),
                repository: "app".into(),
                commit_id: "abc".into(),
                anonymous: false,
                auth_token: Some("ghp_token".into()),
                bundle_type: Some("zip".into()),
            },
            ..local_file_spec("")
        };
        let result = DownloadCommand::build_github_downloader(&spec, Path::new("/tmp/out"), None);
        assert!(result.is_ok());
    }

    #[test]
    fn build_github_downloader_not_github() {
        let result = DownloadCommand::build_github_downloader(
            &local_file_spec("/tmp/x"),
            Path::new("/tmp/out"),
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn build_github_downloader_invalid_bundle_type() {
        let spec = DeploymentSpec {
            revision_source: RevisionSource::GitHub,
            revision: RevisionLocation::GitHub {
                account: "acme".into(),
                repository: "app".into(),
                commit_id: "abc".into(),
                anonymous: true,
                auth_token: None,
                bundle_type: Some("rar".into()),
            },
            ..local_file_spec("")
        };
        let result = DownloadCommand::build_github_downloader(&spec, Path::new("/tmp/out"), None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("bundle_type other than zip or tar"));
    }

    #[test]
    fn missing_appspec_after_download_fails() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));

        // Create source directory without appspec.yml.
        // Uses LocalDirectory path as representative — the validation runs after
        // all download types (S3, GitHub, local file) since it checks the
        // unpacked archive_dir, not the download source.
        let src_dir = dir.path().join("no-appspec");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("script.sh"), "#!/bin/sh").unwrap();

        let spec = local_dir_spec(&src_dir.display().to_string());
        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        fs::create_dir_all(&deploy_dir).unwrap();

        let err = cmd.execute(&spec).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("does not exist at the expected location"),
            "unexpected error: {msg}"
        );
        assert!(msg.contains("appspec.yml"), "error should mention appspec path: {msg}");
    }

    #[test]
    fn execute_github_invalid_bundle_type_fails_fast() {
        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);
        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));

        let deploy_dir = archives.deployment_root_dir("dg-1", "d-123");
        fs::create_dir_all(&deploy_dir).unwrap();

        let spec = DeploymentSpec {
            revision_source: RevisionSource::GitHub,
            revision: RevisionLocation::GitHub {
                account: "acme".into(),
                repository: "app".into(),
                commit_id: "abc".into(),
                anonymous: true,
                auth_token: None,
                bundle_type: Some("rar".into()),
            },
            ..local_file_spec("")
        };

        let result = cmd.execute(&spec);
        assert!(result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn reject_symlinks_in_bundle_rejects_archive_with_symlink() {
        use std::process::Command;

        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);

        let config = AgentConfig {
            hardening: crate::config::HardeningConfig {
                reject_symlinks_in_bundle: true,
                ..Default::default()
            },
            ..AgentConfig::default()
        };
        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(config));

        // Create a tar containing a symlink
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("appspec.yml"), "version: 0.0\nos: linux\n").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.join("evil_link")).unwrap();

        let tar_path = dir.path().join("bundle.tar");
        Command::new("tar")
            .args([
                "-cf",
                &tar_path.display().to_string(),
                "-C",
                &src.display().to_string(),
                ".",
            ])
            .output()
            .unwrap();

        let spec = local_file_spec(&tar_path.display().to_string());

        // Pre-create deploy_dir so LocalFileDownloader can create the bundle symlink in it.
        let deploy_dir =
            archives.deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
        fs::create_dir_all(&deploy_dir).unwrap();

        let result = cmd.execute(&spec);
        assert!(result.is_err(), "should reject bundle with symlink");
        assert!(
            result.as_ref().unwrap_err().to_string().contains("symbolic link"),
            "expected symlink rejection from reject_bundle_symlinks, got: {result:?}"
        );
    }

    // SUID/SGID end-to-end coverage lives in bundle_unpacker unit tests:
    // production `tar -xf` (no `-p`) silently drops SUID/SGID under non-root
    // extraction, so a tar round-trip can't reproduce the bits in CI.

    #[cfg(unix)]
    #[test]
    fn reject_path_traversal_in_bundle_rejects_archive_with_parent_component() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);

        let config = AgentConfig {
            hardening: crate::config::HardeningConfig {
                reject_path_traversal_in_bundle: true,
                ..Default::default()
            },
            ..AgentConfig::default()
        };
        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(config));

        // System `tar -cf` rewrites `../` paths during creation, so hand-write the header.
        let tar_path = dir.path().join("bundle.tar");
        let mut f = fs::File::create(&tar_path).unwrap();
        let body = b"pwned";
        f.write_all(&raw_tar_header_for_test(b"../escape.txt", body.len() as u64))
            .unwrap();
        f.write_all(body).unwrap();
        f.write_all(&vec![0u8; 512 - body.len()]).unwrap();
        let appspec_body = b"version: 0.0\nos: linux\n";
        f.write_all(&raw_tar_header_for_test(b"appspec.yml", appspec_body.len() as u64))
            .unwrap();
        f.write_all(appspec_body).unwrap();
        f.write_all(&vec![0u8; 512 - appspec_body.len()]).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();
        drop(f);

        let spec = local_file_spec(&tar_path.display().to_string());
        let deploy_dir =
            archives.deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
        fs::create_dir_all(&deploy_dir).unwrap();

        let result = cmd.execute(&spec);
        assert!(result.is_err(), "should reject bundle with traversal entry");
        let msg = result.as_ref().unwrap_err().to_string();
        assert!(
            msg.contains("traversal component"),
            "expected traversal rejection from check_path_traversal, got: {msg}"
        );

        let escape_path = deploy_dir.join("escape.txt");
        assert!(!escape_path.exists(), "traversal entry escaped to {}", escape_path.display());
    }

    #[cfg(unix)]
    #[test]
    fn path_traversal_allowed_when_rejection_disabled() {
        use std::io::Write;

        let dir = TempDir::new().unwrap();
        let archives = test_archives(&dir);

        let cmd = DownloadCommand::new(archives.clone(), None, Arc::new(AgentConfig::default()));

        let tar_path = dir.path().join("bundle.tar");
        let mut f = fs::File::create(&tar_path).unwrap();
        let body = b"x";
        f.write_all(&raw_tar_header_for_test(b"../escape.txt", body.len() as u64))
            .unwrap();
        f.write_all(body).unwrap();
        f.write_all(&vec![0u8; 511]).unwrap();
        let appspec_body = b"version: 0.0\nos: linux\n";
        f.write_all(&raw_tar_header_for_test(b"appspec.yml", appspec_body.len() as u64))
            .unwrap();
        f.write_all(appspec_body).unwrap();
        f.write_all(&vec![0u8; 512 - appspec_body.len()]).unwrap();
        f.write_all(&[0u8; 1024]).unwrap();
        drop(f);

        let spec = local_file_spec(&tar_path.display().to_string());
        let deploy_dir =
            archives.deployment_root_dir(&spec.deployment_group_id, &spec.deployment_id);
        fs::create_dir_all(&deploy_dir).unwrap();

        if let Err(e) = cmd.execute(&spec) {
            assert!(
                !e.to_string().contains("traversal"),
                "agent must not gate when flag is off, got: {e}"
            );
        }
    }

    fn raw_tar_header_for_test(name: &[u8], size: u64) -> [u8; 512] {
        let mut header = [0u8; 512];
        let len = name.len().min(100);
        header[..len].copy_from_slice(&name[..len]);
        header[100..107].copy_from_slice(b"0000644");
        header[108..115].copy_from_slice(b"0001000");
        header[116..123].copy_from_slice(b"0001000");
        let size_str = format!("{size:011o}");
        header[124..135].copy_from_slice(size_str.as_bytes());
        header[136..147].copy_from_slice(b"14717450000");
        header[156] = b'0';
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        header[148..156].copy_from_slice(b"        ");
        let cksum: u32 = header.iter().map(|&b| u32::from(b)).sum();
        let cksum_str = format!("{cksum:06o}\0 ");
        header[148..156].copy_from_slice(cksum_str.as_bytes());
        header
    }
}
