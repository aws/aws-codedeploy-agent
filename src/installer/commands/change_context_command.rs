//! `semanage fcontext` command — sets `SELinux` file context.
use crate::application_specification::SeLinuxContext;
use crate::installer::{InstallerError, Result};
use crate::system::{SeLinuxOps, SystemSeLinuxOps};
use serde_json::{Value, json};
use std::io::Write;
use std::path::PathBuf;

/// `SELinux` types that effectively disable MAC enforcement.
const UNCONFINED_BLOCKLIST: &[&str] = &["unconfined_t", "kernel_t", "init_t"];

#[derive(Debug)]
pub struct ChangeContextCommand<S: SeLinuxOps = SystemSeLinuxOps> {
    object: PathBuf,
    context: SeLinuxContext,
    reject_unconfined: bool,
    reject_symlink_target: bool,
    selinux_ops: S,
}

impl ChangeContextCommand<SystemSeLinuxOps> {
    #[must_use]
    pub fn new(
        object: PathBuf,
        context: SeLinuxContext,
        reject_unconfined: bool,
        reject_symlink_target: bool,
    ) -> Self {
        Self {
            object,
            context,
            reject_unconfined,
            reject_symlink_target,
            selinux_ops: SystemSeLinuxOps,
        }
    }
}

impl<S: SeLinuxOps> ChangeContextCommand<S> {
    #[cfg(test)]
    pub fn new_with_ops(
        object: PathBuf,
        context: SeLinuxContext,
        reject_unconfined: bool,
        reject_symlink_target: bool,
        selinux_ops: S,
    ) -> Self {
        Self { object, context, reject_unconfined, reject_symlink_target, selinux_ops }
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, cleanup_file: &mut dyn Write) -> Result<()> {
        if self.context.role().is_some() {
            return Err(InstallerError::SelinuxRoleNotSupported);
        }

        if self.reject_unconfined && UNCONFINED_BLOCKLIST.contains(&self.context.type_()) {
            return Err(InstallerError::UnconfinedSelinuxRejected {
                type_: self.context.type_().to_string(),
            });
        }

        let mut args_vec = vec!["-t", self.context.type_()];
        let user_str;
        let range_str;

        if let Some(user) = self.context.user() {
            user_str = user.to_string();
            args_vec.insert(0, &user_str);
            args_vec.insert(0, "-s");
        }

        if let Some(range) = self.context.range() {
            range_str = range.to_string();
            args_vec.push("-r");
            args_vec.push(&range_str);
        }

        // SECURITY: under `reject_symlink_permission_targets`,
        // semanage/restorecon are external commands with no no-follow option, so
        // canonicalize_no_symlink_swap rejects a symlinked pre-image and verifies
        // the canonical path's device+inode is unchanged (closing the
        // canonicalize TOCTOU window). By default (flag off) semanage/restorecon
        // run on the path as-is, following any symlink — the
        // backwards-compatible behavior.
        let object = if self.reject_symlink_target {
            crate::installer::safe_fs::canonicalize_no_symlink_swap(&self.object)?
        } else {
            self.object.clone()
        };

        self.selinux_ops.set_context(&args_vec, &object).map_err(InstallerError::Io)?;

        self.selinux_ops.restore_context(&object).map_err(InstallerError::Io)?;

        writeln!(cleanup_file, "semanage\0{}", object.display())?;
        Ok(())
    }

    /// Serialize command to JSON hash for debugging/audit
    #[must_use]
    pub fn to_h(&self) -> Value {
        json!({
            "type": "semanage",
            "context": {
                "user": self.context.user(),
                "role": self.context.role(),
                "type": self.context.type_(),
                "range": self.context.range().map(ToString::to_string)
            },
            "file": self.object
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application_specification::{MlsRange, SeLinuxContext};
    use crate::installer::InstallerError;
    use crate::system::MockSeLinuxOps;
    use std::fs;

    #[test]
    fn execute_basic() {
        let file = std::env::temp_dir().join("test_ctx.txt");
        fs::write(&file, "test").unwrap();

        let ctx = SeLinuxContext::new(None, "httpd_sys_content_t".to_string(), None);
        let cmd = ChangeContextCommand::new(file.clone(), ctx, false, false);
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_with_user() {
        let file = std::env::temp_dir().join("test_ctx_user.txt");
        fs::write(&file, "test").unwrap();

        let ctx = SeLinuxContext::new(
            Some("user_u".to_string()),
            "httpd_sys_content_t".to_string(),
            None,
        );
        let cmd = ChangeContextCommand::new(file.clone(), ctx, false, false);
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_with_range() {
        let file = std::env::temp_dir().join("test_ctx_range.txt");
        fs::write(&file, "test").unwrap();

        let range = MlsRange::parse("s0").unwrap();
        let ctx = SeLinuxContext::new(None, "httpd_sys_content_t".to_string(), Some(range));
        let cmd = ChangeContextCommand::new(file.clone(), ctx, false, false);
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[test]
    fn execute_nonexistent() {
        // Hardened path: canonicalize_no_symlink_swap fails deterministically on
        // a missing path regardless of the SystemSeLinuxOps coverage stub.
        let ctx = SeLinuxContext::new(None, "httpd_sys_content_t".to_string(), None);
        let cmd = ChangeContextCommand::new("/nonexistent".into(), ctx, false, true);
        let mut cleanup = Vec::new();
        assert!(cmd.execute(&mut cleanup).is_err());
    }

    #[test]
    fn role_not_supported() {
        let file = std::env::temp_dir().join("test_ctx_role.txt");
        fs::write(&file, "test").unwrap();

        let ctx = SeLinuxContext::new_with_role(
            None,
            Some("object_r".to_string()),
            "httpd_sys_content_t".to_string(),
            None,
        );
        let cmd = ChangeContextCommand::new(file.clone(), ctx, false, false);
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(result.is_err());
        match result.unwrap_err() {
            InstallerError::SelinuxRoleNotSupported => {},
            _ => panic!("Expected SelinuxRoleNotSupported error"),
        }

        fs::remove_file(&file).ok();
    }

    #[test]
    fn semanage_failure() {
        let file = std::env::temp_dir().join("test_ctx_semanage_fail.txt");
        fs::write(&file, "test").unwrap();

        let ctx = SeLinuxContext::new(None, "httpd_sys_content_t".to_string(), None);
        let mock_ops = MockSeLinuxOps::with_failure();
        let cmd = ChangeContextCommand::new_with_ops(file.clone(), ctx, false, false, mock_ops);
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(result.is_err());
        match result.unwrap_err() {
            InstallerError::Io(_) => {},
            _ => panic!("Expected Io error"),
        }

        fs::remove_file(&file).ok();
    }

    #[test]
    fn to_h() {
        let file = std::env::temp_dir().join("test_ctx_to_h.txt");
        fs::write(&file, "test").unwrap();

        let range = MlsRange::parse("s0:c0.c1023").unwrap();
        let ctx = SeLinuxContext::new(
            Some("system_u".to_string()),
            "httpd_sys_content_t".to_string(),
            Some(range),
        );
        let cmd = ChangeContextCommand::new(file.clone(), ctx, false, false);
        let hash = cmd.to_h();

        assert_eq!(hash["type"], "semanage");
        assert_eq!(hash["context"]["user"], "system_u");
        assert_eq!(hash["context"]["type"], "httpd_sys_content_t");
        assert_eq!(hash["context"]["range"], "s0:c0.c1023");
        assert_eq!(hash["file"], file.to_str().unwrap());

        fs::remove_file(&file).ok();
    }

    #[test]
    fn rejects_unconfined_when_flag_enabled() {
        let file = std::env::temp_dir().join("test_ctx_unconfined_reject.txt");
        fs::write(&file, "test").unwrap();

        for type_ in ["unconfined_t", "kernel_t", "init_t"] {
            let ctx = SeLinuxContext::new(None, type_.to_string(), None);
            let cmd = ChangeContextCommand::new_with_ops(
                file.clone(),
                ctx,
                true,
                false,
                MockSeLinuxOps { should_fail: false },
            );
            let mut cleanup = Vec::new();
            let result = cmd.execute(&mut cleanup);

            assert!(result.is_err(), "type {type_} should be rejected");
            match result.unwrap_err() {
                InstallerError::UnconfinedSelinuxRejected { type_: rejected } => {
                    assert_eq!(rejected, type_);
                },
                other => panic!("expected UnconfinedSelinuxRejected for {type_}, got {other:?}"),
            }
        }

        fs::remove_file(&file).ok();
    }

    #[test]
    fn allows_unconfined_when_flag_disabled() {
        let file = std::env::temp_dir().join("test_ctx_unconfined_allow.txt");
        fs::write(&file, "test").unwrap();

        let ctx = SeLinuxContext::new(None, "unconfined_t".to_string(), None);
        let cmd = ChangeContextCommand::new_with_ops(
            file.clone(),
            ctx,
            false,
            false,
            MockSeLinuxOps { should_fail: false },
        );
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(result.is_ok(), "unconfined_t must be accepted when flag is disabled");

        fs::remove_file(&file).ok();
    }

    #[test]
    fn safe_type_passes_when_flag_enabled() {
        let file = std::env::temp_dir().join("test_ctx_safe_with_flag.txt");
        fs::write(&file, "test").unwrap();

        let ctx = SeLinuxContext::new(None, "httpd_sys_content_t".to_string(), None);
        let cmd = ChangeContextCommand::new_with_ops(
            file.clone(),
            ctx,
            true,
            false,
            MockSeLinuxOps { should_fail: false },
        );
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(result.is_ok(), "safe types must be accepted even when flag is enabled");

        fs::remove_file(&file).ok();
    }
}
