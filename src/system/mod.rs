//! @risk low
//!
//! System abstraction layer — file ops, process ops, `SELinux`, env vars.
pub mod env_ops;
pub mod file_ops;
#[cfg(unix)]
pub mod linux_ops;
pub mod process_ops;
pub mod secure_files;
pub mod version_file;
#[cfg(unix)]
pub mod selinux_ops;

pub use env_ops::{EnvOps, SystemEnvOps};
pub use file_ops::{PlatformFileOperations, SystemFileOperations, ensure_executable};
#[cfg(unix)]
pub use linux_ops::{LinuxOps, SystemLinuxOps};
pub use process_ops::kill_process_group;
pub use secure_files::{
    agent_file_mode, create_deployment_dir, create_dir_secure, create_dir_world_readable,
    create_file_secure, write_file_secure,
};
#[cfg(unix)]
pub use selinux_ops::{SeLinuxOps, SystemSeLinuxOps};

#[cfg(test)]
pub use env_ops::MockEnvOps;
#[cfg(test)]
pub use file_ops::MockFileOperations;
#[cfg(all(test, unix))]
pub use linux_ops::MockLinuxOps;
#[cfg(all(test, unix))]
pub use selinux_ops::MockSeLinuxOps;
