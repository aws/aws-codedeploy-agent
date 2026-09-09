//!
//! Archive reuse for bounce (restart) deployments.
//!
//! When the deployment spec carries `ReuseArchiveFromDeploymentId`, the bundle for
//! the new deployment can be materialised from the referenced deployment's on-host
//! archive instead of being downloaded again — the point of a bounce.
//!
//! Two rules govern everything here:
//!
//! 1. **Fail closed to a download.** Every failure path returns an error so the
//!    caller falls back to the normal (revision-pinned) download. Reuse is an
//!    optimisation; it is never the only way to get the bytes.
//! 2. **Never leave a partial archive.** A half-copied archive that got deployed
//!    would be worse than any download. On failure the destination is torn down
//!    before returning, so the fallback starts from clean state.
//!
//! Hard links and copies, never symlinks: `cleanup_old_archives` prunes the source
//! deployment once `last_successful` moves on, which would leave a symlinked archive
//! dangling. A hard link keeps the bundle bytes alive independently of the source
//! directory entry.

use crate::host_command::DeploymentArchives;
use std::fs;
use std::io;
use std::path::Path;
use tracing::{debug, warn};

/// `BUNDLE_SOURCE_FILE` value when the archive was reused from a prior deployment.
pub const SOURCE_ARCHIVE_REUSE: &str = "archive-reuse";

/// `BUNDLE_SOURCE_FILE` value when the bundle was fetched from its revision source.
pub const SOURCE_DOWNLOADED: &str = "downloaded";

/// Upper bound on a deployment ID, so a hostile value cannot build an absurd path.
/// Real IDs are `d-` plus nine characters; this leaves generous headroom.
const MAX_DEPLOYMENT_ID_LEN: usize = 64;

/// Whether `id` is a well-formed deployment ID that is safe to use as a single
/// path component.
///
/// Accepts `d-` followed by one or more uppercase-alphanumeric characters. That
/// rejects everything dangerous for path construction — `/`, `\`, `..`, `.`, NUL,
/// absolute paths, empty strings — by allowing only a known-good character set
/// rather than blocklisting separators.
///
/// A `false` result means "do not reuse", never "fail the deployment".
#[must_use]
pub fn is_valid_deployment_id(id: &str) -> bool {
    let Some(suffix) = id.strip_prefix("d-") else {
        return false;
    };
    !suffix.is_empty()
        && id.len() <= MAX_DEPLOYMENT_ID_LEN
        && suffix.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// Materialise the new deployment's bundle from `source_deployment_id`'s archive.
///
/// On success the destination holds a `deployment-archive/` copy and, when the
/// source had one, a hard-linked `bundle.tar`. The caller may then proceed exactly
/// as if it had downloaded, including the `AppSpec` presence check.
///
/// # Errors
/// Returns an error — after removing any partial destination state — if the ID is
/// malformed, the source archive is missing or unusable, or any filesystem
/// operation fails. Every such error means "fall back to downloading".
pub fn reuse_archive(
    archives: &DeploymentArchives,
    group_id: &str,
    source_deployment_id: &str,
    dest_deployment_id: &str,
    app_spec_path: &str,
) -> io::Result<()> {
    if !is_valid_deployment_id(source_deployment_id) {
        return Err(io::Error::other(format!(
            "Refusing to reuse archive: {source_deployment_id:?} is not a well-formed deployment ID"
        )));
    }
    // Guard against reusing our own directory: the caller wipes the destination
    // archive, which for a self-reference would destroy the very source.
    if source_deployment_id == dest_deployment_id {
        return Err(io::Error::other(
            "Refusing to reuse archive: source and destination deployment are the same",
        ));
    }

    let source_archive = archives.archive_dir(group_id, source_deployment_id);
    let source_bundle = archives.artifact_bundle_path(group_id, source_deployment_id);
    let dest_archive = archives.archive_dir(group_id, dest_deployment_id);
    let dest_bundle = archives.artifact_bundle_path(group_id, dest_deployment_id);

    // Completeness check before touching the destination. A pruned or half-written
    // source is the expected case on a host that joined after the source deployment,
    // or once retention has caught up with it.
    if !source_archive.is_dir() {
        return Err(io::Error::other(format!(
            "Cannot reuse archive: {} is not a directory",
            source_archive.display()
        )));
    }
    if fs::read_dir(&source_archive)?.next().is_none() {
        return Err(io::Error::other(format!(
            "Cannot reuse archive: {} is empty",
            source_archive.display()
        )));
    }

    // An archive can exist yet be unusable -- interrupted copy, partially pruned
    // tree. Without this the deployment would proceed and then fail on the
    // AppSpec check downstream, which fails the host instead of falling back to a
    // download. Treat a missing AppSpec as "incomplete, do not reuse".
    // Defence in depth: the spec builder already rejects an unsafe AppSpec path at ingest, but this
    // function is reached with a caller-supplied value and builds a path from it, so it does not
    // rely on that. Same shared check, so the two paths cannot drift apart.
    if !crate::system::file_ops::is_safe_relative_path(app_spec_path) {
        return Err(io::Error::other(format!(
            "Cannot reuse archive: unsafe AppSpec path {app_spec_path:?}"
        )));
    }

    let source_appspec = source_archive.join(app_spec_path);
    if !source_appspec.exists() {
        return Err(io::Error::other(format!(
            "Cannot reuse archive: incomplete, no AppSpec at {}",
            source_appspec.display()
        )));
    }

    // Defence in depth: the validated ID cannot traverse, but confirm the resolved
    // source really sits under this group's directory before copying from it.
    let group_dir = archives.deployment_root_dir(group_id, "").canonicalize()?;
    let resolved_source = source_archive.canonicalize()?;
    if !resolved_source.starts_with(&group_dir) {
        return Err(io::Error::other(format!(
            "Cannot reuse archive: {} resolves outside {}",
            resolved_source.display(),
            group_dir.display()
        )));
    }

    match copy_into_place(&source_archive, &source_bundle, &dest_archive, &dest_bundle) {
        Ok(()) => {
            carry_bundle_etag_forward(archives, group_id, source_deployment_id, dest_deployment_id);
            debug!(
                source = %source_deployment_id,
                dest = %dest_deployment_id,
                "Reused deployment archive"
            );
            Ok(())
        },
        Err(e) => {
            // Tear down whatever we managed to write, so the download fallback does
            // not inherit a partial archive.
            warn!(
                source = %source_deployment_id,
                "Archive reuse failed, discarding partial state: {e}"
            );
            if dest_archive.exists()
                && let Err(rm) = fs::remove_dir_all(&dest_archive)
            {
                warn!(path = %dest_archive.display(), "Failed to clean partial archive: {rm}");
            }
            if dest_bundle.exists()
                && let Err(rm) = fs::remove_file(&dest_bundle)
            {
                warn!(path = %dest_bundle.display(), "Failed to clean partial bundle: {rm}");
            }
            Err(e)
        },
    }
}

/// Copy the source deployment's `.bundle-etag` alongside the reused archive.
///
/// The marker is how `LifecycleEventExecutor` resolves `BUNDLE_ETAG` for hooks when the deployment
/// spec carries no eTag of its own, which is the common case. Reuse does not download, so without
/// this a restart would leave hooks with `BUNDLE_ETAG` unset even though the revision is byte for
/// byte the one the source deployment recorded -- a behaviour change for customer scripts, not an
/// intended part of the reuse optimisation. Copying it also keeps a chain of bounces intact, each
/// handing the value on.
///
/// Best-effort: `BUNDLE_ETAG` is diagnostic metadata, so a missing or unreadable marker must not
/// fail an otherwise healthy deployment.
fn carry_bundle_etag_forward(
    archives: &DeploymentArchives,
    group_id: &str,
    source_deployment_id: &str,
    dest_deployment_id: &str,
) {
    let source = archives
        .deployment_root_dir(group_id, source_deployment_id)
        .join(crate::host_command::BUNDLE_ETAG_FILE);
    if !source.exists() {
        return;
    }

    let dest = archives
        .deployment_root_dir(group_id, dest_deployment_id)
        .join(crate::host_command::BUNDLE_ETAG_FILE);
    if let Err(e) = fs::copy(&source, &dest) {
        warn!(
            source = %source.display(),
            dest = %dest.display(),
            "Failed to carry the bundle ETag forward to the reused deployment: {e}"
        );
    }
}

fn copy_into_place(
    source_archive: &Path,
    source_bundle: &Path,
    dest_archive: &Path,
    dest_bundle: &Path,
) -> io::Result<()> {
    if let Some(parent) = dest_archive.parent() {
        fs::create_dir_all(parent)?;
    }
    if dest_archive.exists() {
        fs::remove_dir_all(dest_archive)?;
    }
    crate::system::file_ops::copy_dir_recursive(source_archive, dest_archive)?;

    // A LocalDirectory revision never produces a bundle.tar, so its absence is
    // normal rather than an error — the archive alone is what Install consumes.
    // Clear any destination bundle first, unconditionally. An earlier failed attempt on
    // this same deployment can have left a bundle.tar behind, and when the source has no
    // bundle of its own we would otherwise keep the stale one next to a freshly copied
    // archive -- a mismatched pair, while still returning Ok. That would break this
    // module's "never leave a partial archive" guarantee.
    if dest_bundle.exists() {
        fs::remove_file(dest_bundle)?;
    }

    if source_bundle.exists() {
        fs::hard_link(source_bundle, dest_bundle)?;
    }
    Ok(())
}

/// Record how the bundle was obtained, for per-host diagnostics. Best-effort: a
/// write failure must not fail an otherwise healthy deployment.
pub fn record_bundle_source(deploy_dir: &Path, source: &str, restrict_permissions: bool) {
    let path = deploy_dir.join(crate::host_command::BUNDLE_SOURCE_FILE);
    if let Err(e) = crate::system::write_file_secure(
        &path,
        source.as_bytes(),
        crate::system::agent_file_mode(restrict_permissions),
    ) {
        warn!(path = %path.display(), "Failed to record bundle source marker: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn archives(dir: &TempDir) -> DeploymentArchives {
        let root = dir.path().join("deployments");
        let instructions = dir.path().join("instructions");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&instructions).unwrap();
        DeploymentArchives::new(root, instructions, 5)
    }

    /// Build a usable source deployment: archive dir with an appspec, plus a bundle.
    fn seed_source(a: &DeploymentArchives, group: &str, id: &str) {
        let archive = a.archive_dir(group, id);
        fs::create_dir_all(archive.join("scripts")).unwrap();
        fs::write(archive.join("appspec.yml"), "version: 0.0\nos: linux\n").unwrap();
        fs::write(archive.join("scripts/start.sh"), "#!/bin/sh\n").unwrap();
        fs::write(a.artifact_bundle_path(group, id), "bundle-bytes").unwrap();
    }

    #[test]
    fn accepts_real_deployment_id_shapes() {
        assert!(is_valid_deployment_id("d-A1B2C3D4E"));
        assert!(is_valid_deployment_id("d-0"));
        assert!(is_valid_deployment_id("d-ABCDEFGHI"));
    }

    #[test]
    fn rejects_path_separators_and_traversal() {
        for bad in [
            "d-../../etc",
            "d-A/B",
            "d-A\\B",
            "../d-ABC",
            "/d-ABC",
            "d-A.B",
            "d-",
            "",
            "d-abc", // lowercase is not a real ID shape
            "not-an-id",
            "d-A B",
            "d-A\0B",
        ] {
            assert!(!is_valid_deployment_id(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn rejects_absurdly_long_id() {
        let long = format!("d-{}", "A".repeat(MAX_DEPLOYMENT_ID_LEN));
        assert!(!is_valid_deployment_id(&long));
    }

    #[test]
    fn reuses_archive_and_hard_links_bundle() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        seed_source(&a, "dg-1", "d-PREV");

        reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "appspec.yml").unwrap();

        let dest = a.archive_dir("dg-1", "d-NEW");
        assert!(dest.join("appspec.yml").exists(), "appspec must be present for Install");
        assert!(dest.join("scripts/start.sh").exists(), "nested content must be copied");
        assert!(a.artifact_bundle_path("dg-1", "d-NEW").exists());

        // Hard link, not symlink — a symlink would dangle once the source is pruned.
        let bundle = a.artifact_bundle_path("dg-1", "d-NEW");
        assert!(!bundle.is_symlink(), "bundle must not be a symlink");
        assert_eq!(fs::read_to_string(&bundle).unwrap(), "bundle-bytes");
    }

    #[test]
    fn reused_archive_survives_source_pruning() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        seed_source(&a, "dg-1", "d-PREV");

        reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "appspec.yml").unwrap();

        // Simulate cleanup_old_archives reclaiming the source deployment.
        fs::remove_dir_all(a.deployment_root_dir("dg-1", "d-PREV")).unwrap();

        let dest = a.archive_dir("dg-1", "d-NEW");
        assert!(dest.join("appspec.yml").exists(), "copied archive must be independent");
        let bundle = a.artifact_bundle_path("dg-1", "d-NEW");
        assert_eq!(
            fs::read_to_string(&bundle).unwrap(),
            "bundle-bytes",
            "hard-linked bundle must outlive the source directory entry"
        );
    }

    #[test]
    fn missing_source_archive_is_an_error() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        let err = reuse_archive(&a, "dg-1", "d-GONE", "d-NEW", "appspec.yml").unwrap_err();
        assert!(err.to_string().contains("not a directory"), "got: {err}");
    }

    #[test]
    fn empty_source_archive_is_an_error() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        fs::create_dir_all(a.archive_dir("dg-1", "d-PREV")).unwrap();

        let err = reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "appspec.yml").unwrap_err();
        assert!(err.to_string().contains("is empty"), "got: {err}");
    }

    #[test]
    fn malformed_id_is_rejected_before_touching_disk() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);

        let err = reuse_archive(&a, "dg-1", "d-../escape", "d-NEW", "appspec.yml").unwrap_err();
        assert!(err.to_string().contains("not a well-formed deployment ID"), "got: {err}");
        assert!(
            !a.deployment_root_dir("dg-1", "d-NEW").exists(),
            "nothing may be created for a rejected ID"
        );
    }

    #[test]
    fn self_reference_is_rejected() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        seed_source(&a, "dg-1", "d-SAME");

        let err = reuse_archive(&a, "dg-1", "d-SAME", "d-SAME", "appspec.yml").unwrap_err();
        assert!(err.to_string().contains("same"), "got: {err}");
        // The source must still be intact — this is the case that would destroy it.
        assert!(a.archive_dir("dg-1", "d-SAME").join("appspec.yml").exists());
    }

    #[test]
    fn source_without_bundle_still_reuses_archive() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        // LocalDirectory revisions have no bundle.tar at all.
        let archive = a.archive_dir("dg-1", "d-PREV");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("appspec.yml"), "version: 0.0\n").unwrap();

        reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "appspec.yml").unwrap();

        assert!(a.archive_dir("dg-1", "d-NEW").join("appspec.yml").exists());
        assert!(
            !a.artifact_bundle_path("dg-1", "d-NEW").exists(),
            "no bundle should be invented when the source had none"
        );
    }

    #[test]
    fn incomplete_archive_without_appspec_is_an_error() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        // Non-empty, but no AppSpec: an interrupted copy or partially pruned tree.
        let archive = a.archive_dir("dg-1", "d-PREV");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("some-file.txt"), "x").unwrap();

        let err = reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "appspec.yml").unwrap_err();
        assert!(err.to_string().contains("incomplete"), "got: {err}");
        assert!(
            !a.archive_dir("dg-1", "d-NEW").exists(),
            "must reject before copying anything to the destination"
        );
    }

    #[test]
    fn rejects_appspec_path_escaping_the_archive() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        seed_source(&a, "dg-1", "d-PREV");

        for bad in ["../../etc/passwd", "/etc/passwd", "nested/../../escape.yml"] {
            let err = reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", bad).unwrap_err();
            assert!(err.to_string().contains("unsafe AppSpec path"), "{bad}: {err}");
        }
        assert!(!a.archive_dir("dg-1", "d-NEW").exists());
    }

    #[test]
    fn accepts_nested_appspec_path() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        let archive = a.archive_dir("dg-1", "d-PREV");
        fs::create_dir_all(archive.join("configs")).unwrap();
        fs::write(
            archive.join("configs/appspec.yml"),
            "version: 0.0
",
        )
        .unwrap();

        reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "configs/appspec.yml").unwrap();
        assert!(a.archive_dir("dg-1", "d-NEW").join("configs/appspec.yml").exists());
    }

    #[test]
    fn honours_custom_appspec_path() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        let archive = a.archive_dir("dg-1", "d-PREV");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("custom.yml"), "version: 0.0\n").unwrap();

        // Default name is absent, so the default lookup must reject...
        assert!(reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "appspec.yml").is_err());
        // ...while the spec's actual AppSpec name is accepted.
        reuse_archive(&a, "dg-1", "d-PREV", "d-NEW2", "custom.yml").unwrap();
        assert!(a.archive_dir("dg-1", "d-NEW2").join("custom.yml").exists());
    }

    #[test]
    fn stale_destination_bundle_is_cleared_when_source_has_none() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        // LocalDirectory-style source: archive only, no bundle.tar of its own.
        let archive = a.archive_dir("dg-1", "d-PREV");
        fs::create_dir_all(&archive).unwrap();
        fs::write(archive.join("appspec.yml"), "version: 0.0\n").unwrap();

        // An earlier failed attempt on this deployment left a bundle behind.
        let dest_bundle = a.artifact_bundle_path("dg-1", "d-NEW");
        fs::create_dir_all(dest_bundle.parent().unwrap()).unwrap();
        fs::write(&dest_bundle, "stale-bytes").unwrap();

        reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "appspec.yml").unwrap();

        assert!(
            !dest_bundle.exists(),
            "a stale bundle must not survive next to a reused archive"
        );
        assert!(a.archive_dir("dg-1", "d-NEW").join("appspec.yml").exists());
    }

    #[test]
    fn existing_destination_archive_is_replaced() {
        let dir = TempDir::new().unwrap();
        let a = archives(&dir);
        seed_source(&a, "dg-1", "d-PREV");

        let dest = a.archive_dir("dg-1", "d-NEW");
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("stale.txt"), "old").unwrap();

        reuse_archive(&a, "dg-1", "d-PREV", "d-NEW", "appspec.yml").unwrap();

        assert!(!dest.join("stale.txt").exists(), "stale content must be cleared");
        assert!(dest.join("appspec.yml").exists());
    }

    /// Hooks read `BUNDLE_ETAG` from this marker when the spec carries no `eTag`, so a reused
    /// deployment has to leave one behind exactly as a downloaded one does.
    #[test]
    fn carries_the_bundle_etag_forward_on_reuse() {
        let dir = TempDir::new().unwrap();
        let archives = archives(&dir);
        seed_source(&archives, "dg-1", "d-SRC");
        fs::write(
            archives
                .deployment_root_dir("dg-1", "d-SRC")
                .join(crate::host_command::BUNDLE_ETAG_FILE),
            "abc123",
        )
        .unwrap();

        reuse_archive(&archives, "dg-1", "d-SRC", "d-DEST", "appspec.yml").unwrap();

        assert_eq!(
            fs::read_to_string(
                archives
                    .deployment_root_dir("dg-1", "d-DEST")
                    .join(crate::host_command::BUNDLE_ETAG_FILE)
            )
            .unwrap(),
            "abc123"
        );
    }

    /// A source with no marker is normal -- an older agent, or a GitHub revision. Reuse must still
    /// succeed rather than fail on missing diagnostic metadata.
    #[test]
    fn reuse_succeeds_when_the_source_has_no_bundle_etag() {
        let dir = TempDir::new().unwrap();
        let archives = archives(&dir);
        seed_source(&archives, "dg-1", "d-SRC");

        reuse_archive(&archives, "dg-1", "d-SRC", "d-DEST", "appspec.yml").unwrap();

        assert!(
            !archives
                .deployment_root_dir("dg-1", "d-DEST")
                .join(crate::host_command::BUNDLE_ETAG_FILE)
                .exists()
        );
    }

    #[test]
    fn records_bundle_source_marker() {
        let dir = TempDir::new().unwrap();
        let deploy_dir = dir.path().join("d-NEW");
        fs::create_dir_all(&deploy_dir).unwrap();

        record_bundle_source(&deploy_dir, SOURCE_ARCHIVE_REUSE, false);

        let marker = deploy_dir.join(crate::host_command::BUNDLE_SOURCE_FILE);
        assert_eq!(fs::read_to_string(marker).unwrap(), SOURCE_ARCHIVE_REUSE);
    }

    #[test]
    fn marker_values_are_distinguishable() {
        assert_ne!(SOURCE_ARCHIVE_REUSE, SOURCE_DOWNLOADED);
    }
}
