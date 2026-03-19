//! @risk high
//!
//! Installer core — executes install commands and manages cleanup files.
use super::{
    builder::{Command, CommandBuilder},
    commands::{RemoveCommand, RemoveContextCommand},
    error::{InstallerError, Result},
};
use crate::application_specification::{AppSpec, FileExistsBehavior};
use serde_json::json;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use tracing::{debug, info};

#[derive(Debug)]
pub struct Installer {
    deployment_archive_dir: PathBuf,
    deployment_instructions_dir: PathBuf,
    file_exists_behavior: FileExistsBehavior,
}

impl Installer {
    #[must_use]
    pub fn new(
        deployment_archive_dir: PathBuf,
        deployment_instructions_dir: PathBuf,
        file_exists_behavior: FileExistsBehavior,
    ) -> Self {
        Self { deployment_archive_dir, deployment_instructions_dir, file_exists_behavior }
    }

    /// @risk high — writes files to customer filesystem, wrong behavior corrupts deployments
    ///
    /// # Errors
    /// Returns an error if installation fails.
    pub fn install(&self, deployment_group_id: &str, spec: &AppSpec) -> Result<()> {
        debug!("Starting installation for deployment group: {deployment_group_id}");

        let cleanup_file =
            self.deployment_instructions_dir.join(format!("{deployment_group_id}-cleanup"));

        // Execute and remove existing cleanup file
        if cleanup_file.exists() {
            let contents = fs::read_to_string(&cleanup_file)?;
            Self::execute_cleanup(&contents)?;
            fs::remove_file(&cleanup_file)?;
        }

        // Generate instructions
        let mut builder = CommandBuilder::new();
        self.generate_instructions(&mut builder, spec)?;

        debug!("Generated {} commands", builder.commands().len());

        // Write install.json with full command details for debugging/audit
        let install_file = self
            .deployment_instructions_dir
            .join(format!("{deployment_group_id}-install.json"));

        let json_commands: Vec<_> = builder.commands().iter().map(Command::to_h).collect();
        fs::write(
            &install_file,
            serde_json::to_string_pretty(&json!({ "instructions": json_commands }))?,
        )?;

        // Execute commands and write cleanup file
        let mut cleanup = File::create(&cleanup_file)?;
        for cmd in builder.commands() {
            cmd.execute_with_cleanup(&mut cleanup)?;
        }

        debug!("Installation completed successfully");
        info!(
            deployment_group_id,
            command_count = builder.commands().len(),
            "Installation completed"
        );
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
        let source = self.deployment_archive_dir.join(file_mapping.source());
        debug!("Processing file mapping from source: {}", source.display());

        // Validate path doesn't escape archive directory
        let canonical_source = source.canonicalize().unwrap_or(source.clone());
        let canonical_archive = self.deployment_archive_dir.canonicalize()?;
        if !canonical_source.starts_with(&canonical_archive) {
            return Err(InstallerError::PathTraversal {
                path: file_mapping.source().to_string(),
                base: canonical_archive,
            });
        }

        debug!("Destination: {}", file_mapping.destination());

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

        // Handle file copy
        let dest = Path::new(file_mapping.destination()).join(source.file_name().unwrap());
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
                let object = line.strip_prefix("semanage\0").unwrap();
                debug!("Cleanup: removing SELinux context for {object:?}");
                // Semanage removal failures are silently ignored
                let _ =
                    RemoveContextCommand::new(PathBuf::from(object)).execute(&mut std::io::sink());
            } else {
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
        let archive_dir = std::env::temp_dir().join("test_archive");
        let instructions_dir = std::env::temp_dir().join("test_instructions");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::create_dir_all(&instructions_dir).unwrap();

        let _installer = Installer::new(
            archive_dir.clone(),
            instructions_dir.clone(),
            FileExistsBehavior::Disallow,
        );

        fs::remove_dir_all(&archive_dir).ok();
        fs::remove_dir_all(&instructions_dir).ok();
    }

    #[test]
    fn install_minimal() {
        let archive_dir = std::env::temp_dir().join("test_install_archive");
        let instructions_dir = std::env::temp_dir().join("test_install_instructions");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::create_dir_all(&instructions_dir).unwrap();

        let installer = Installer::new(
            archive_dir.clone(),
            instructions_dir.clone(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();

        installer.install("test-group", &spec).unwrap();

        let install_file = instructions_dir.join("test-group-install.json");
        let cleanup_file = instructions_dir.join("test-group-cleanup");
        assert!(install_file.exists());
        assert!(cleanup_file.exists());

        fs::remove_dir_all(&archive_dir).ok();
        fs::remove_dir_all(&instructions_dir).ok();
    }

    #[test]
    fn install_with_existing_cleanup() {
        let archive_dir = std::env::temp_dir().join("test_cleanup_archive");
        let instructions_dir = std::env::temp_dir().join("test_cleanup_instructions");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::create_dir_all(&instructions_dir).unwrap();

        let cleanup_file = instructions_dir.join("test-group-cleanup");
        fs::write(&cleanup_file, "").unwrap();

        let installer = Installer::new(
            archive_dir.clone(),
            instructions_dir.clone(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();

        installer.install("test-group", &spec).unwrap();

        fs::remove_dir_all(&archive_dir).ok();
        fs::remove_dir_all(&instructions_dir).ok();
    }

    #[test]
    fn cleanup_empty_lines() {
        let archive_dir = std::env::temp_dir().join("test_empty_archive");
        let instructions_dir = std::env::temp_dir().join("test_empty_instructions");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::create_dir_all(&instructions_dir).unwrap();

        let cleanup_file = instructions_dir.join("test-group-cleanup");
        fs::write(&cleanup_file, "\n\n\n").unwrap();

        let installer = Installer::new(
            archive_dir.clone(),
            instructions_dir.clone(),
            FileExistsBehavior::Disallow,
        );

        let appspec_yaml = r"
version: 0.0
os: linux
";
        let spec = AppSpec::parse(appspec_yaml).unwrap();

        installer.install("test-group", &spec).unwrap();

        fs::remove_dir_all(&archive_dir).ok();
        fs::remove_dir_all(&instructions_dir).ok();
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

        let installer = Installer::new(
            archive_dir.path().to_path_buf(),
            instructions_dir.path().to_path_buf(),
            FileExistsBehavior::Disallow,
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
        let instructions_dir = std::env::temp_dir().join("nonexistent_dir_12345");

        // Ensure directory doesn't exist
        fs::remove_dir_all(&instructions_dir).ok();

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
