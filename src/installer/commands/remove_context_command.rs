//! `SELinux` context removal command — removes fcontext mappings during cleanup.
use crate::installer::Result;
use crate::system::{SeLinuxOps, SystemSeLinuxOps};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug)]
pub struct RemoveContextCommand<S: SeLinuxOps = SystemSeLinuxOps> {
    object: PathBuf,
    selinux_ops: S,
}

impl RemoveContextCommand<SystemSeLinuxOps> {
    #[must_use]
    pub fn new(object: PathBuf) -> Self {
        Self { object, selinux_ops: SystemSeLinuxOps }
    }
}

impl<S: SeLinuxOps> RemoveContextCommand<S> {
    #[cfg(test)]
    pub fn new_with_ops(object: PathBuf, selinux_ops: S) -> Self {
        Self { object, selinux_ops }
    }

    /// # Errors
    /// Returns an error if the command execution fails.
    pub fn execute(&self, _cleanup_file: &mut dyn Write) -> Result<()> {
        self.selinux_ops.remove_context(&self.object).map_err(Into::into)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::InstallerError;
    use crate::system::MockSeLinuxOps;
    use std::fs;

    #[test]
    fn execute() {
        let file = std::env::temp_dir().join("test_rmctx.txt");
        fs::write(&file, "test").unwrap();

        let cmd = RemoveContextCommand::new(file.clone());
        let mut cleanup = Vec::new();
        let _ = cmd.execute(&mut cleanup);

        fs::remove_file(&file).ok();
    }

    #[test]
    fn semanage_failure() {
        let file = std::env::temp_dir().join("test_rmctx_fail.txt");
        fs::write(&file, "test").unwrap();

        let mock_ops = MockSeLinuxOps::with_failure();
        let cmd = RemoveContextCommand::new_with_ops(file.clone(), mock_ops);
        let mut cleanup = Vec::new();
        let result = cmd.execute(&mut cleanup);

        assert!(result.is_err());
        match result.unwrap_err() {
            InstallerError::Io(_) => {},
            _ => panic!("Expected Io error"),
        }

        fs::remove_file(&file).ok();
    }
}
