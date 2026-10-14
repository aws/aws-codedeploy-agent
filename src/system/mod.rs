//! System abstraction layer — file ops, process ops, `SELinux`, env vars.
//!
//! `linux_ops` (setfacl) and `selinux_ops` (semanage/restorecon) shell out to
//! Linux-only tools, so they are gated on `target_os = "linux"`, not `unix`;
//! macOS takes the rejection path in `installer::builder` instead.
pub mod env_ops;
pub mod file_ops;
#[cfg(target_os = "linux")]
pub mod linux_ops;
pub mod process_ops;
pub mod secure_files;
#[cfg(target_os = "linux")]
pub mod selinux_ops;
pub mod version_file;

#[cfg(unix)]
pub use env_ops::DYNAMIC_LOADER_ENV_VARS;
pub use env_ops::{EnvOps, SystemEnvOps};
pub use file_ops::{PlatformFileOperations, SystemFileOperations, ensure_executable, write_all_at};
#[cfg(target_os = "linux")]
pub use linux_ops::{LinuxOps, SystemLinuxOps};
pub use process_ops::kill_process_group;
pub use secure_files::{
    agent_file_mode, create_deployment_dir, create_dir_secure, create_dir_world_readable,
    create_file_secure, write_file_secure,
};
#[cfg(target_os = "linux")]
pub use selinux_ops::{SeLinuxOps, SystemSeLinuxOps};

#[cfg(test)]
pub use env_ops::MockEnvOps;
#[cfg(test)]
pub use file_ops::MockFileOperations;
#[cfg(all(test, target_os = "linux"))]
pub use linux_ops::MockLinuxOps;
#[cfg(all(test, target_os = "linux"))]
pub use selinux_ops::MockSeLinuxOps;
