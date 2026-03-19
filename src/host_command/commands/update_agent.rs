//! @risk medium
//!
//! `UpdateDeploymentAgent` command.
//!
//! Handles the agent self-update host command. The actual update logic
//! (package manager detection, download, install, restart) is implemented
//! in T11.2/T11.3. This module provides the command routing and stub.
//!
//! Ruby reference: `command_executor.rb` — `UpdateDeploymentAgent` triggers
//! the agent updater script rather than going through the normal deployment
//! spec flow (no appspec, no file installation).
//!
//! Update logs will be written to `UPDATER_LOG_PATH` (see `src/logging/mod.rs`).

use std::io;
use tracing::info;

#[derive(Debug, Default)]
pub struct UpdateAgentCommand;

impl UpdateAgentCommand {
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// Execute the `UpdateDeploymentAgent` command.
    ///
    /// Currently a stub — actual update logic comes in T11.3.
    ///
    /// # Errors
    /// Returns an error if the update process fails.
    pub fn execute(&self) -> io::Result<Vec<String>> {
        info!("UpdateDeploymentAgent command received — update logic not yet implemented");
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_creates_command() {
        let cmd = UpdateAgentCommand::new();
        // Debug impl works
        let _ = format!("{cmd:?}");
    }

    #[test]
    fn execute_returns_empty_vec() {
        let cmd = UpdateAgentCommand::new();
        let result = cmd.execute().unwrap();
        assert!(result.is_empty());
    }
}
