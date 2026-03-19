//! @risk low
//!
//! System abstraction layer — file ops, process ops, `SELinux`, env vars.
pub mod env_ops;
pub mod file_ops;
pub mod linux_ops;
pub mod process_ops;
pub mod selinux_ops;

pub use env_ops::{EnvOps, SystemEnvOps};
pub use file_ops::{PlatformFileOperations, SystemFileOperations, ensure_executable};
pub use linux_ops::{LinuxOps, SystemLinuxOps};
pub use process_ops::kill_process_group;
pub use selinux_ops::{SeLinuxOps, SystemSeLinuxOps};

#[cfg(test)]
pub use env_ops::MockEnvOps;
#[cfg(test)]
pub use file_ops::MockFileOperations;
#[cfg(test)]
pub use linux_ops::MockLinuxOps;
#[cfg(test)]
pub use selinux_ops::MockSeLinuxOps;
