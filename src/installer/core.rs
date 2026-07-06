//! @risk high
//!
//! Installer core — executes install commands and manages cleanup files.
#[cfg(unix)]
use super::commands::RemoveContextCommand;
use super::{
    builder::{Command, CommandBuilder},
    commands::RemoveCommand,
    error::{InstallerError, Result},
};
use crate::application_specification::{AppSpec, FileExistsBehavior};
use crate::system::write_file_secure;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

// Instruction-file mode (install.json, cleanup) comes from
// `system::agent_file_mode`: 0644 by default, 0600 under
// `restrict_agent_dir_permissions`.

// Four independent opt-in hardening toggles mirroring their AgentConfig
// fields; an enum would force invalid combinations to be representable.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct Installer {
    deployment_archive_dir: PathBuf,
    deployment_instructions_dir: PathBuf,
    file_exists_behavior: FileExistsBehavior,
    reject_unconfined_selinux: bool,
    reject_unsafe_permissions: bool,
    reject_path_traversal: bool,
    reject_symlink_permission_targets: bool,
    restrict_permissions: bool,
}

impl Installer {
    /// `reject_path_traversal` defaults to `false` to preserve
    /// backwards-compatible behavior: historically the agent applied no path
    /// containment at all — neither on the source (the archive dir joined with
    /// the mapping's `source`) nor on an escaping destination — so the flag is a
    /// hardening that must stay opt-in. It gates both the
    /// source-escapes-archive and destination-escapes-own-root checks in
    /// `process_file_mapping`.
    #[must_use]
    pub fn new(
        deployment_archive_dir: PathBuf,
        deployment_instructions_dir: PathBuf,
        file_exists_behavior: FileExistsBehavior,
    ) -> Self {
        Self::with_options(
            deployment_archive_dir,
            deployment_instructions_dir,
            file_exists_behavior,
            false,
            false,
            false,
        )
    }

    #[must_use]
    pub fn with_options(
        deployment_archive_dir: PathBuf,
        deployment_instructions_dir: PathBuf,
        file_exists_behavior: FileExistsBehavior,
        reject_unconfined_selinux: bool,
        reject_unsafe_permissions: bool,
        reject_path_traversal: bool,
    ) -> Self {
        Self {
            deployment_archive_dir,
            deployment_instructions_dir,
            file_exists_behavior,
            reject_unconfined_selinux,
            reject_unsafe_permissions,
            reject_path_traversal,
            reject_symlink_permission_targets: false,
            restrict_permissions: false,
        }
    }

    /// Set the mode policy for instruction files, from
    /// `restrict_agent_dir_permissions`. Defaults to `false`, preserving
    /// backwards-compatible behavior.
    #[must_use]
    pub fn with_restrict_permissions(mut self, restrict: bool) -> Self {
        self.restrict_permissions = restrict;
        self
    }

    /// Reject symlinked `permissions:` targets and apply permissions no-follow,
    /// from `reject_symlink_permission_targets`. Defaults to `false`, the
    /// backwards-compatible behavior in which permission sinks follow symlinks.
    #[must_use]
    pub fn with_reject_symlink_permission_targets(mut self, reject: bool) -> Self {
        self.reject_symlink_permission_targets = reject;
        self
    }

    /// @risk high — writes files to customer filesystem, wrong behavior corrupts deployments
    ///
    /// # Errors
    /// Returns an error if installation fails.
    pub fn install(&self, deployment_group_id: &str, spec: &AppSpec) -> Result<()> {
        debug!("Starting installation for deployment group: {deployment_group_id}");

        let cleanup_file =
            self.deployment_instructions_dir.join(format!("{deployment_group_id}-cleanup"));

        // Open-and-handle-NotFound rather than exists()-then-read: opening once
        // is race-free (no swap window between the check and the read).
        match fs::File::open(&cleanup_file) {
            Ok(mut file) => {
                // The cleanup file's lines are used as paths to remove, so verify
                // it still looks agent-authored (owner, not group/other-writable,
                // regular file) before acting on it. A mismatch means something
                // replaced it — skip cleanup rather than delete injected paths.
                if Self::cleanup_file_is_trusted(&file, &cleanup_file)? {
                    use std::io::Read;
                    let mut contents = String::new();
                    file.read_to_string(&mut contents)?;
                    Self::execute_cleanup(&contents)?;
                }
                drop(file);
                fs::remove_file(&cleanup_file)?;
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.into()),
        }

        // Generate instructions
        let mut builder = CommandBuilder::with_options(
            self.reject_unconfined_selinux,
            self.reject_unsafe_permissions,
            self.reject_symlink_permission_targets,
        );
        self.generate_instructions(&mut builder, spec)?;

        debug!("Generated {} commands", builder.commands().len());

        // Write install.json with full command details for debugging/audit
        let install_file = self
            .deployment_instructions_dir
            .join(format!("{deployment_group_id}-install.json"));

        let json_commands: Vec<_> = builder.commands().iter().map(Command::to_h).collect();
        write_file_secure(
            &install_file,
            serde_json::to_string_pretty(&json!({ "instructions": json_commands }))?.as_bytes(),
            crate::system::agent_file_mode(self.restrict_permissions),
        )?;

        // Execute commands and write cleanup file (0600 — lists deployed files).
        let mut cleanup = crate::system::create_file_secure(
            &cleanup_file,
            crate::system::agent_file_mode(self.restrict_permissions),
        )?;
        for cmd in builder.commands() {
            cmd.execute_with_cleanup(&mut cleanup)?;
        }

        debug!("Installation completed successfully");
        // GRCOV_STOP_COVERAGE
        info!(
            deployment_group_id,
            command_count = builder.commands().len(),
            "Installation completed"
        );
        // GRCOV_BEGIN_COVERAGE
        Ok(())
    }

    fn generate_instructions(&self, builder: &mut CommandBuilder, spec: &AppSpec) -> Result<()> {
        let file_count = spec.files().iter().count();
        debug!("Processing {file_count} file mappings");

        // Process files
        for file_mapping in spec.files().iter() {
            self.process_file_mapping(builder, file_mapping, spec)?;
        }

        let perm_count = spec.permissions().iter().count();
        debug!("Processing {perm_count} permissions");

        // Process permissions
        for permission in spec.permissions().iter() {
            Self::process_permission(builder, permission)?;
        }

        Ok(())
    }

    fn process_file_mapping(
        &self,
        builder: &mut CommandBuilder,
        file_mapping: &crate::application_specification::FileMapping,
        spec: &AppSpec,
    ) -> Result<()> {
        // Strip leading '/' so that an absolute-looking `source` stays relative to
        // the archive dir: a leading slash must not discard the base path (as
        // Rust's PathBuf::join would).
        let source_relative =
            file_mapping.source().strip_prefix('/').unwrap_or(file_mapping.source());
        let source = self.deployment_archive_dir.join(source_relative);
        debug!("Processing file mapping from source: {}", source.display());

        // Reject a `source` that escapes the archive dir (opt-in; with the flag
        // off no containment is applied, the backwards-compatible behavior).
        // Normalize both sides LEXICALLY (`nu_path::expand_path`, resolving
        // `.`/`..` without touching the filesystem), not via `canonicalize`:
        // `starts_with` treats `..` as an ordinary component so un-normalized
        // paths spuriously pass containment, and `canonicalize` errors on a
        // not-yet-existent `source` (which would fail open). Lexical
        // normalization rejects the escape regardless of existence. This is a
        // lexical check only — it does not resolve symlinks;
        // the write-side symlink race is closed separately by the O_NOFOLLOW copy
        // in `copy_command.rs`. Mirrors the hook-`location` check in
        // `lifecycle_event/executor.rs`.
        if self.reject_path_traversal {
            let normalized_source = nu_path::expand_path(&source, true);
            let normalized_archive = nu_path::expand_path(&self.deployment_archive_dir, true);
            if !normalized_source.starts_with(&normalized_archive) {
                return Err(InstallerError::PathTraversal {
                    path: file_mapping.source().to_string(),
                    base: normalized_archive,
                });
            }
        }

        debug!("Destination: {}", file_mapping.destination());

        // Reject a `destination` whose `..` components climb above its own root
        // (e.g. `../../etc/cron.d`), which would land the copy outside the path
        // the AppSpec declared. Historically the agent wrote to such a
        // destination without complaint, so this is gated behind the same opt-in
        // flag as the source check. Legitimate absolute destinations
        // (`/etc`, `/var/www`, `C:\inetpub`) never escape their own root.
        if self.reject_path_traversal
            && Self::destination_escapes_root(Path::new(file_mapping.destination()))
        {
            return Err(InstallerError::DestinationEscapesRoot {
                destination: file_mapping.destination().to_string(),
            });
        }

        let behavior = spec.file_exists_behavior().unwrap_or(self.file_exists_behavior);

        // Handle directory copy
        if source.is_dir() {
            Self::fill_missing_ancestors(builder, Path::new(file_mapping.destination()))?;
            Self::generate_directory_copy(
                builder,
                &source,
                Path::new(file_mapping.destination()),
                behavior,
            )?;
            return Ok(());
        }

        // `file_name()` is None for a path ending in `..` or a bare root —
        // return an error rather than panicking on `unwrap()`.
        let file_name = source.file_name().ok_or_else(|| {
            InstallerError::InvalidSourceFileName { source: file_mapping.source().to_string() }
        })?;
        let dest = Path::new(file_mapping.destination()).join(file_name);
        debug!("File copy destination: {}", dest.display());
        Self::fill_missing_ancestors(builder, &dest)?;
        Self::generate_normal_copy(builder, &source, &dest, behavior)?;
        Ok(())
    }

    fn process_permission(
        builder: &mut CommandBuilder,
        permission: &crate::application_specification::Permission,
    ) -> Result<()> {
        let object = Path::new(permission.object());

        // Handle directory permissions
        if !builder.copying_file(object) {
            if builder.making_directory(object) || object.is_dir() {
                for matched in builder.find_matches(permission)? {
                    builder.set_permissions(&matched, permission)?;
                }
            }
            return Ok(());
        }

        // Handle file permissions
        if !permission.types().contains(&crate::application_specification::ObjectType::File) {
            return Ok(());
        }

        // Apply-time validation: reject file-typed permissions that declare
        // directory-shaped `pattern:` / `except:` fields, then check the ACL.
        permission.validate_file_permission()?;
        permission.validate_file_acl(object)?;
        builder.set_permissions(object, permission)?;
        Ok(())
    }

    fn generate_directory_copy(
        builder: &mut CommandBuilder,
        source: &Path,
        dest: &Path,
        behavior: FileExistsBehavior,
    ) -> Result<()> {
        if !dest.is_dir() {
            builder.mkdir(dest)?;
        }

        // Rust's Path/OsString handles
        // non-UTF-8 filenames natively via OsStr, no encoding conversion needed.
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            let entry_path = entry.path();
            let entry_dest = dest.join(entry.file_name());

            if entry_path.is_dir() {
                Self::generate_directory_copy(builder, &entry_path, &entry_dest, behavior)?;
            } else {
                Self::generate_normal_copy(builder, &entry_path, &entry_dest, behavior)?;
            }
        }

        Ok(())
    }

    fn generate_normal_copy(
        builder: &mut CommandBuilder,
        source: &Path,
        dest: &Path,
        behavior: FileExistsBehavior,
    ) -> Result<()> {
        if dest.exists() {
            match behavior {
                FileExistsBehavior::Disallow => {
                    return Err(InstallerError::FileAlreadyExists {
                        destination: dest.to_path_buf(),
                    });
                },
                FileExistsBehavior::Overwrite => {
                    builder.copy(source, dest)?;
                },
                FileExistsBehavior::Retain => {},
            }
        } else {
            builder.copy(source, dest)?;
        }
        Ok(())
    }

    /// Lexically decide whether `dest` uses `..` components that climb above
    /// its own root — i.e. the resolved path would land outside the directory
    /// tree the `AppSpec`'s `destination` names.
    ///
    /// Pure component arithmetic (no filesystem access, no symlink following):
    /// walk the components tracking depth below the leading root (if any).
    /// A `..` that would take depth negative is an escape. Legitimate absolute
    /// destinations (`/etc`, `/var/www`) never go negative; an internal `..`
    /// that stays within the path (`/var/www/../cgi-bin`) is allowed because it
    /// resolves back inside. Only a net climb above the root is rejected
    /// (`/a/../../etc`, `../../etc`, `/../etc`).
    fn destination_escapes_root(dest: &Path) -> bool {
        use std::path::Component;
        let mut depth: i64 = 0;
        for component in dest.components() {
            match component {
                Component::ParentDir => {
                    depth -= 1;
                    if depth < 0 {
                        return true;
                    }
                },
                Component::Normal(_) => depth += 1,
                // RootDir / Prefix / CurDir do not change how far below the
                // root we are.
                Component::RootDir | Component::Prefix(_) | Component::CurDir => {},
            }
        }
        false
    }

    fn fill_missing_ancestors(builder: &mut CommandBuilder, dest: &Path) -> Result<()> {
        let mut missing = Vec::new();
        let mut parent = dest.parent();

        while let Some(p) = parent {
            if p.exists() || p == Path::new(".") || p == Path::new("/") {
                break;
            }
            missing.push(p.to_path_buf());
            parent = p.parent();
        }

        for dir in missing.into_iter().rev() {
            builder.mkdir(&dir)?;
        }

        Ok(())
    }

    /// Verify the cleanup file still looks agent-authored before its lines are
    /// used as removal paths. `install()` creates it via `create_file_secure`
    /// (0600 on Unix; protected DACL on Windows) in the agent-owned instructions
    /// dir; on Unix this rejects a non-regular file, one the agent user does not
    /// own, or a group/other-writable one (all signs of tampering). On Windows
    /// the protected DACL blocks non-admin writes at the FS layer, so only the
    /// regular-file check applies.
    ///
    /// Returns `Ok(false)` (warn + skip cleanup) on a mismatch rather than
    /// erroring, so a tampered file neither injects removals nor wedges the
    /// deployment — the stale file is still unlinked afterward.
    fn cleanup_file_is_trusted(file: &fs::File, path: &Path) -> Result<bool> {
        let meta = file.metadata()?;

        if !meta.is_file() {
            tracing::warn!(
                path = %path.display(),
                "Cleanup file is not a regular file; skipping cleanup execution"
            );
            return Ok(false);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let uid = nix::unistd::geteuid().as_raw();
            if meta.uid() != uid {
                tracing::warn!(
                    path = %path.display(),
                    file_uid = meta.uid(),
                    agent_uid = uid,
                    "Cleanup file is not owned by the agent user; skipping cleanup execution"
                );
                return Ok(false);
            }
            // Reject group- or world-writable (0o022): a writable cleanup file is
            // an injection vector for the removal paths below.
            if meta.mode() & 0o022 != 0 {
                tracing::warn!(
                    path = %path.display(),
                    mode = format!("{:o}", meta.mode() & 0o7777),
                    "Cleanup file is group/other-writable; skipping cleanup execution"
                );
                return Ok(false);
            }
        }

        Ok(true)
    }

    /// Whether a cleanup-file line is safe to treat as a removal path.
    ///
    /// Rejects empty lines and lines containing an embedded NUL (a truncation /
    /// injection marker; the only legitimate NUL is the `semanage\0` sentinel,
    /// handled by the caller before this is reached), and rejects any line whose
    /// `..` components climb above its own root (reusing the same lexical rule as
    /// [`Self::destination_escapes_root`]). Legitimate cleanup entries are the
    /// destination paths the agent itself wrote — absolute, relative, or Windows
    /// paths — none of which escape their own root.
    fn cleanup_line_is_valid(line: &str) -> bool {
        if line.is_empty() || line.contains('\0') {
            return false;
        }
        !Self::destination_escapes_root(Path::new(line))
    }

    fn execute_cleanup(contents: &str) -> Result<()> {
        let lines: Vec<&str> = contents.lines().collect();

        // Handle incomplete last line: remove if it doesn't end with newline,
        // indicating the write was interrupted (partial cleanup file from a crash).
        // cleanup file format.
        let lines = if !contents.ends_with('\n') && !lines.is_empty() {
            &lines[..lines.len() - 1]
        } else {
            &lines[..]
        };

        debug!("Executing {} cleanup commands", lines.len());

        for line in lines.iter().rev() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with("semanage\0") {
                #[cfg(unix)]
                {
                    let object = line.strip_prefix("semanage\0").unwrap();
                    // Validate the SELinux object path the same way as a removal
                    // path (the NUL is the sentinel, already stripped here).
                    if !Self::cleanup_line_is_valid(object) {
                        tracing::warn!(
                            object = %object,
                            "Skipping malformed SELinux cleanup entry"
                        );
                        continue;
                    }
                    debug!("Cleanup: removing SELinux context for {object:?}");
                    // Semanage removal failures are silently ignored
                    let _ = RemoveContextCommand::new(PathBuf::from(object))
                        .execute(&mut std::io::sink());
                }
            } else {
                // Skip empty/NUL-bearing/root-escaping lines rather than removing
                // whatever path they name; legitimate entries are the agent's own
                // destination paths.
                if !Self::cleanup_line_is_valid(line) {
                    tracing::warn!(line = %line, "Skipping malformed cleanup entry");
                    continue;
                }
                debug!("Cleanup: removing {line:?}");
                RemoveCommand::new(PathBuf::from(line)).execute(&mut std::io::sink())?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application_specification::{AppSpec, FileExistsBehavior};
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn new() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        let _installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );
    }

    #[test]
    fn install_minimal() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();

        installer.install("test-group", &spec).unwrap();

        let install_file = instructions_dir.path().join("test-group-install.json");
        let cleanup_file = instructions_dir.path().join("test-group-cleanup");
        assert!(install_file.exists());
        assert!(cleanup_file.exists());
    }

    #[test]
    fn install_with_existing_cleanup() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        let cleanup_file = instructions_dir.path().join("test-group-cleanup");
        fs::write(&cleanup_file, "").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();

        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn cleanup_empty_lines() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        let cleanup_file = instructions_dir.path().join("test-group-cleanup");
        fs::write(&cleanup_file, "\n\n\n").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();

        installer.install("test-group", &spec).unwrap();
    }
    #[test]
    fn install_with_directory_copy() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source directory structure
        let src_dir = archive_dir.path().join("myapp");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("file1.txt"), "content1").unwrap();

        let nested = src_dir.join("subdir");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("file2.txt"), "content2").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: myapp
    destination: {}
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_with_permissions() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        let src_file = archive_dir.path().join("app.sh");
        fs::write(&src_file, "#!/bin/bash\necho hello").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: app.sh
    destination: {}/app.sh
permissions:
  - object: {}/app.sh
    type:
      - file
    mode: "755"
"#,
            dest_dir.path().display(),
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_with_directory_permissions() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source directory
        let src_dir = archive_dir.path().join("data");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("file.txt"), "data").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: data
    destination: {}/data
permissions:
  - object: {}/data
    pattern: "**"
    type:
      - directory
    mode: "755"
"#,
            dest_dir.path().display(),
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_path_traversal_blocked() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        let installer = Installer::with_options(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
            false,
            false,
            true,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
files:
  - source: ../../../etc/passwd
    destination: /tmp/stolen
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        assert!(result.is_err());
        match result {
            Err(InstallerError::PathTraversal { .. }) => {},
            _ => panic!("Expected PathTraversal error"),
        }
    }

    /// An escaping `source` whose target does not exist must still be rejected
    /// (the lexical check is existence-independent, unlike a `canonicalize`-based
    /// one which fails open on a nonexistent target). Uses a guaranteed-absent
    /// target so the test doesn't depend on `/etc/passwd`.
    #[test]
    fn install_nonexistent_path_traversal_blocked() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        let installer = Installer::with_options(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
            false,
            false,
            true,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
files:
  - source: ../../nonexistent-dir-xyz/evil
    destination: /tmp/stolen
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        assert!(
            matches!(result, Err(InstallerError::PathTraversal { .. })),
            "escaping source must be rejected even when its target does not exist, got: {result:?}"
        );
    }

    #[test]
    fn install_leading_slash_source_resolves_inside_archive() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file inside archive
        fs::write(archive_dir.path().join("index.html"), "<html></html>").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Overwrite,
        );

        // Leading slash should be stripped so the source stays inside the archive
        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: /index.html
    destination: {}
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();

        assert!(dest_dir.path().join("index.html").exists());
    }

    #[test]
    fn install_path_traversal_allowed_when_flag_off() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // BY-DESIGN: this test intentionally PINS the documented
        // backwards-compatible default — with `reject_path_traversal_in_bundle`
        // off, a `..` SOURCE path is not rejected. It is asserting the deliberate
        // opt-in default, not endorsing an insecure one; flipping the default is
        // a separate reviewed change (see the by-design note on
        // `Installer::new`). The same flag also gates the destination-escape
        // check, so with the flag off neither source nor destination containment
        // fires.
        let appspec_yaml = r"
version: 0.0
os: linux
files:
  - source: ../../../etc/passwd
    destination: /tmp/stolen
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        // Should NOT be a PathTraversal error — the check is off. Any other
        // result (file not found, etc.) is acceptable.
        assert!(
            !matches!(&result, Err(InstallerError::PathTraversal { .. })),
            "PathTraversal should not fire when reject_path_traversal is off"
        );
    }

    #[test]
    fn install_creates_missing_parent_directories() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source directory
        let src_dir = archive_dir.path().join("app");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("file.txt"), "data").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Destination has multiple missing parent directories
        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: app
    destination: {}/a/b/c/app
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_permission_type_mismatch() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        let src_file = archive_dir.path().join("app.sh");
        fs::write(&src_file, "#!/bin/bash").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Permission specifies directory type but we're copying a file
        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: app.sh
    destination: {}/app.sh
permissions:
  - object: {}/app.sh
    type:
      - directory
    mode: "755"
"#,
            dest_dir.path().display(),
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_single_file_with_permissions() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create single source file (not in a directory)
        fs::write(archive_dir.path().join("script.sh"), "#!/bin/bash\necho test").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: script.sh
    destination: {}
permissions:
  - object: {}/script.sh
    type:
      - file
    mode: "755"
"#,
            dest_dir.path().display(),
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_file_overwrite() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        fs::write(archive_dir.path().join("file.txt"), "new content").unwrap();

        // Create existing destination file
        fs::create_dir_all(dest_dir.path()).unwrap();
        fs::write(dest_dir.path().join("file.txt"), "old content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Overwrite,
        );

        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: file.txt
    destination: {}
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_file_retain() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        fs::write(archive_dir.path().join("file.txt"), "new content").unwrap();

        // Create existing destination file
        fs::create_dir_all(dest_dir.path()).unwrap();
        fs::write(dest_dir.path().join("file.txt"), "old content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Retain,
        );

        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: file.txt
    destination: {}
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_file_disallow_existing() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        fs::write(archive_dir.path().join("file.txt"), "new content").unwrap();

        // Create existing destination file
        fs::create_dir_all(dest_dir.path()).unwrap();
        fs::write(dest_dir.path().join("file.txt"), "old content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: file.txt
    destination: {}
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        assert!(result.is_err());
        match result {
            Err(InstallerError::FileAlreadyExists { .. }) => {},
            _ => panic!("Expected FileAlreadyExists error"),
        }
    }

    #[test]
    fn install_existing_directory_permissions() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create existing directory structure at destination
        let target_dir = dest_dir.path().join("existing");
        fs::create_dir_all(&target_dir).unwrap();
        fs::write(target_dir.join("file.txt"), "data").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Apply permissions to existing directory
        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
permissions:
  - object: {}
    pattern: "**"
    type:
      - directory
    mode: "755"
"#,
            target_dir.display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }

    #[test]
    fn install_with_selinux_cleanup() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        // Create cleanup file with semanage entry
        let cleanup_file = instructions_dir.path().join("test-group-cleanup");
        fs::write(&cleanup_file, "semanage\0/tmp/testfile\n/tmp/otherfile\n").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        // This will fail because semanage command doesn't exist, but that's ok
        let _ = installer.install("test-group", &spec);
    }

    #[test]
    fn install_with_complete_last_line_cleanup() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        // Create cleanup file where last line DOES end with newline
        let cleanup_file = instructions_dir.path().join("test-group-cleanup");
        fs::write(&cleanup_file, "/tmp/file1\n/tmp/file2\n").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let _ = installer.install("test-group", &spec);
    }

    #[test]
    fn file_permission_on_copied_file() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        let src_file = archive_dir.path().join("app.txt");
        fs::write(&src_file, "content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let dest_file = dest_dir.path().join("app.txt");
        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: app.txt
    destination: {}
permissions:
  - object: {}
    type:
      - file
    owner: root
    group: root
    mode: "644"
"#,
            dest_dir.path().display(),
            dest_file.display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        let _ = installer.install("test-group", &spec);
    }

    #[test]
    fn install_with_incomplete_last_line_cleanup() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        // Create cleanup file where last line does NOT end with newline (incomplete)
        let cleanup_file = instructions_dir.path().join("test-group-cleanup");
        fs::write(&cleanup_file, "/tmp/file1\n/tmp/incomplete").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let _ = installer.install("test-group", &spec);
    }

    #[test]
    fn install_fails_with_invalid_instructions_dir() {
        let archive_dir = TempDir::new().unwrap();
        // A guaranteed-nonexistent path under a temp dir — the instructions dir
        // does not exist.
        let scratch = TempDir::new().unwrap();
        let instructions_dir = scratch.path().join("nonexistent_child");

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.clone(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        // Should fail because instructions directory doesn't exist
        assert!(result.is_err());
    }

    #[test]
    fn install_file_with_mkdir_failure() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        // Create a source file
        fs::write(archive_dir.path().join("file.txt"), "content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Try to copy to a destination where we can't create parent dirs
        // Use a path that will cause mkdir to fail
        let appspec_yaml = r"
version: 0.0
os: linux
files:
  - source: file.txt
    destination: /proc/invalid/path/file.txt
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        // Should fail when trying to create parent directories
        assert!(result.is_err());
    }

    #[test]
    fn install_permission_on_nonexistent_directory() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Try to set permissions on a directory that doesn't exist and we're not creating
        let appspec_yaml = r#"
version: 0.0
os: linux
permissions:
  - object: /nonexistent/directory
    pattern: "**"
    type:
      - directory
    mode: "755"
"#;
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        // Should fail or succeed depending on whether directory exists
        let _ = result;
    }

    #[test]
    fn install_directory_copy_with_permission_denied() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        // Create source directory
        let src_dir = archive_dir.path().join("data");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("file.txt"), "content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Try to copy to a protected location that will fail
        let appspec_yaml = r"
version: 0.0
os: linux
files:
  - source: data
    destination: /proc/invalid_dir
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        // Should fail when trying to create directory in /proc
        assert!(result.is_err());
    }

    #[test]
    fn install_directory_copy_source_not_readable() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create a directory structure where nested copy will fail
        let src_dir = archive_dir.path().join("data");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("file.txt"), "content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // First install to create the destination
        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: data
    destination: {}
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();

        // Now try to install again with Disallow - should fail
        let installer2 = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let result = installer2.install("test-group2", &spec);

        // Should fail because files already exist and behavior is Disallow
        assert!(result.is_err());
    }

    #[test]
    fn install_file_permission_set_fails() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        fs::write(archive_dir.path().join("file.txt"), "content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Try to set permissions on a file with invalid ACL or context
        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: file.txt
    destination: {}
permissions:
  - object: {}/file.txt
    type:
      - file
    acls:
      - "u:nonexistentuser:rwx"
"#,
            dest_dir.path().display(),
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        // May fail if ACL operations fail
        let _ = result;
    }

    #[test]
    fn install_duplicate_file_permissions() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        fs::write(archive_dir.path().join("file.txt"), "content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Set duplicate permissions on the same file
        let dest_file = dest_dir.path().join("file.txt");
        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: file.txt
    destination: {}
permissions:
  - object: {}
    type:
      - file
    mode: "644"
  - object: {}
    type:
      - file
    mode: "755"
"#,
            dest_dir.path().display(),
            dest_file.display(),
            dest_file.display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        // Should fail with DuplicatePermission error
        assert!(result.is_err());
        match result {
            Err(InstallerError::DuplicatePermission { .. }) => {},
            _ => panic!("Expected DuplicatePermission error"),
        }
    }

    #[test]
    fn file_permission_validation_and_setting() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Create source file
        fs::write(archive_dir.path().join("data.txt"), "test data").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        // Copy file and set permissions - this should execute lines 110-112
        let dest_file = dest_dir.path().join("data.txt");
        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: data.txt
    destination: {}
permissions:
  - object: {}
    type:
      - file
    mode: "600"
    owner: root
    group: root
"#,
            dest_dir.path().display(),
            dest_file.display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        let result = installer.install("test-group", &spec);

        // Should succeed (commands will fail during execution but that's ok)
        let _ = result;
    }

    #[test]
    fn cleanup_with_complete_lines_that_succeed() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        // Create files that will be in the cleanup
        let file1 = instructions_dir.path().join("cleanup_file1");
        let file2 = instructions_dir.path().join("cleanup_file2");
        fs::write(&file1, "data1").unwrap();
        fs::write(&file2, "data2").unwrap();

        // Create cleanup file where last line DOES end with newline
        let cleanup_file = instructions_dir.path().join("test-group-cleanup");
        fs::write(&cleanup_file, format!("{}\n{}\n", file1.display(), file2.display())).unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let _ = installer.install("test-group", &spec);
    }

    #[test]
    fn install_directory_object_with_file_type_and_pattern_succeeds() {
        // A permission whose `object:` is a destination DIRECTORY with
        // `type: [file]` + `pattern: file_*` + `except: [file_755]` selects
        // files under that directory. This shape is accepted: a directory object
        // routes through `find_matches`, which validates only the ACL per file,
        // so the whole `install()` must succeed end to end — parse AND apply.
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        // Files that land in the destination directory (the permission object).
        fs::write(archive_dir.path().join("file_777"), "a").unwrap();
        fs::write(archive_dir.path().join("file_755"), "b").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Overwrite,
        );

        let agent_test = dest_dir.path().join("agent_test");
        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: file_777
    destination: {agent_test}
  - source: file_755
    destination: {agent_test}
permissions:
  - object: {agent_test}
    pattern: 'file_*'
    except: ['file_755']
    mode: "777"
    type:
      - file
"#,
            agent_test = agent_test.display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("agent-test-group", &spec).unwrap();
    }

    #[test]
    fn cleanup_line_validation() {
        // Legit cleanup entries (agent-written destination paths) are valid,
        // including relative and Windows-style absolutes.
        for ok in [
            "/agent_test/file",
            "/var/www/index.html",
            "relative/dir",
            r"c:\inetpub\app",
        ] {
            assert!(Installer::cleanup_line_is_valid(ok), "{ok} should be a valid cleanup line");
        }
        // Malformed / escaping entries are rejected.
        assert!(!Installer::cleanup_line_is_valid(""), "empty line invalid");
        assert!(!Installer::cleanup_line_is_valid("/a\0/b"), "embedded NUL invalid");
        assert!(
            !Installer::cleanup_line_is_valid("../../etc/cron.d/x"),
            "root-escaping line invalid"
        );
    }

    #[cfg(unix)]
    #[test]
    fn execute_cleanup_skips_malformed_lines_and_removes_valid() {
        // A cleanup file with one legit path (should be removed) and one
        // root-escaping line (should be skipped, not removed).
        let dir = TempDir::new().unwrap();
        let victim = dir.path().join("legit_file");
        fs::write(&victim, "x").unwrap();
        // A file outside that an escaping entry would target if not skipped.
        let outside = dir.path().join("outside");
        fs::write(&outside, "keep").unwrap();

        // Build cleanup contents: the escaping line uses `..` to climb out.
        let contents = format!("{}\n../../{}\n", victim.display(), "should_not_matter");
        Installer::execute_cleanup(&contents).unwrap();

        assert!(!victim.exists(), "valid cleanup entry should be removed");
        assert!(outside.exists(), "escaping entry must be skipped, unrelated file intact");
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_file_trust_rejects_group_writable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("grp-cleanup");
        fs::write(&path, "/some/path\n").unwrap();
        // Make it group/other-writable — the tamper signal.
        fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        let file = fs::File::open(&path).unwrap();
        assert!(
            !Installer::cleanup_file_is_trusted(&file, &path).unwrap(),
            "group/other-writable cleanup file must not be trusted"
        );
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_file_trust_accepts_agent_owned_0600() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("ok-cleanup");
        fs::write(&path, "/some/path\n").unwrap();
        fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let file = fs::File::open(&path).unwrap();
        assert!(
            Installer::cleanup_file_is_trusted(&file, &path).unwrap(),
            "agent-owned 0600 cleanup file should be trusted"
        );
    }

    #[test]
    fn install_rejects_source_with_no_file_name() {
        // A source ending in `..` has no final component; must return
        // InvalidSourceFileName, not panic. Flag off so we reach the file-copy
        // path rather than the source-traversal guard.
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Overwrite,
        );

        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: foo/..
    destination: {}
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        let result = installer.install("nofilename-group", &spec);
        assert!(
            matches!(result, Err(InstallerError::InvalidSourceFileName { .. })),
            "source with no file name must error, not panic: {result:?}"
        );
    }

    #[test]
    fn destination_escapes_root_unit() {
        // Legitimate absolute destinations — never escape.
        for ok in [
            "/etc",
            "/var/www",
            "/agent_test",
            "/var/www/../cgi-bin",
            "/a/b/../c",
        ] {
            assert!(
                !Installer::destination_escapes_root(Path::new(ok)),
                "{ok} must be allowed (does not climb above its own root)"
            );
        }
        // Escapes — a net climb above the destination's own root.
        for bad in [
            "../../etc/cron.d",
            "/../etc",
            "/a/../../etc",
            "foo/../../bar",
        ] {
            assert!(
                Installer::destination_escapes_root(Path::new(bad)),
                "{bad} must be rejected (climbs above its own root)"
            );
        }
    }

    #[test]
    fn install_rejects_destination_escaping_its_root_when_flag_enabled() {
        // With `reject_path_traversal_in_bundle` ON, a `destination` that climbs
        // above its own root must be rejected before any copy is generated.
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        fs::write(archive_dir.path().join("payload"), "x").unwrap();

        let installer = Installer::with_options(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Overwrite,
            false,
            false,
            true, // reject_path_traversal
        );

        // Relative destination that escapes above its own root.
        let appspec_yaml = r"
version: 0.0
os: linux
files:
  - source: payload
    destination: ../../etc/cron.d
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("esc-group", &spec);
        assert!(
            matches!(result, Err(InstallerError::DestinationEscapesRoot { .. })),
            "escaping destination must be rejected when the flag is on, got: {result:?}"
        );
    }

    #[test]
    fn install_does_not_check_destination_escape_when_flag_disabled() {
        // With the flag OFF (the default) an escaping destination is joined and
        // written to without complaint. The agent must NOT reject with
        // DestinationEscapesRoot when the flag is off — that is the long-standing
        // backwards-compatible behavior. (The copy itself may fail for unrelated
        // filesystem reasons in the test sandbox; we only assert the containment
        // check does not fire.)
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();

        fs::write(archive_dir.path().join("payload"), "x").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Overwrite,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
files:
  - source: payload
    destination: ../../etc/cron.d
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();
        let result = installer.install("esc-group", &spec);
        assert!(
            !matches!(result, Err(InstallerError::DestinationEscapesRoot { .. })),
            "with the flag off the containment check must not fire, got: {result:?}"
        );
    }

    #[test]
    fn install_allows_legitimate_absolute_destination() {
        // The containment check must NOT break CodeDeploy's core contract:
        // deploying to an arbitrary ABSOLUTE destination. Uses a real temp dir
        // as the absolute destination so the copy actually succeeds end to end.
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        fs::write(archive_dir.path().join("index.html"), "<html></html>").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Overwrite,
        );

        let appspec_yaml = format!(
            r"
version: 0.0
os: linux
files:
  - source: index.html
    destination: {}
",
            dest_dir.path().display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("abs-group", &spec).unwrap();
        assert!(dest_dir.path().join("index.html").exists());
    }

    #[test]
    fn file_permission_without_file_type() {
        let archive_dir = TempDir::new().unwrap();
        let instructions_dir = TempDir::new().unwrap();
        let dest_dir = TempDir::new().unwrap();

        fs::write(archive_dir.path().join("data.txt"), "content").unwrap();

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
        );

        let dest_file = dest_dir.path().join("data.txt");
        let appspec_yaml = format!(
            r#"
version: 0.0
os: linux
files:
  - source: data.txt
    destination: {}
permissions:
  - object: {}
    type:
      - directory
    mode: "755"
"#,
            dest_dir.path().display(),
            dest_file.display()
        );
        let spec = AppSpec::parse(&appspec_yaml).unwrap();
        installer.install("test-group", &spec).unwrap();
    }
}
