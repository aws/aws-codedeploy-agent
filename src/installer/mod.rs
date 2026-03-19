//! @risk high
//!
//! File installer — copies files from deployment archive to their destinations.
//!
//! Generates a sequence of commands (copy, mkdir, chmod, chown, setfacl, semanage)
//! from the appspec file mappings, executes them, and writes a cleanup file for
//! rollback on the next deployment.

pub mod builder;
pub mod commands;
pub mod core;
pub mod error;

pub use builder::{Command, CommandBuilder};
pub use core::Installer;
pub use error::{InstallerError, Result};
