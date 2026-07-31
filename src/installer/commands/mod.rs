//! Installer command types — copy, mkdir, chmod, chown, setfacl, semanage.
#[cfg(unix)]
mod change_acl_command;
#[cfg(unix)]
mod change_context_command;
#[cfg(unix)]
mod change_mode_command;
#[cfg(unix)]
mod change_owner_command;
mod copy_command;
mod make_directory_command;
mod remove_command;
#[cfg(unix)]
mod remove_context_command;

#[cfg(unix)]
pub use change_acl_command::ChangeAclCommand;
#[cfg(unix)]
pub use change_context_command::ChangeContextCommand;
#[cfg(unix)]
pub use change_mode_command::ChangeModeCommand;
#[cfg(unix)]
pub use change_owner_command::ChangeOwnerCommand;
pub use copy_command::CopyCommand;
pub use make_directory_command::MakeDirectoryCommand;
pub use remove_command::RemoveCommand;
#[cfg(unix)]
pub use remove_context_command::RemoveContextCommand;
