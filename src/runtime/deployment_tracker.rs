//! @risk medium
//!
//! Deployment tracker trait and types
//!
//! Defines the interface for tracking active deployments. Implementations
//! manage deployment lifecycle state including start, stop, and querying
//! active deployments.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeploymentTrackerError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid deployment ID: {0}")]
    InvalidDeploymentId(String),
}

#[derive(Debug, Clone)]
pub struct ActiveDeployment {
    pub deployment_id: String,
    pub host_command_identifier: String,
    pub timestamp: u64,
}

/// Trait for tracking active deployments to prevent concurrent executions
pub trait DeploymentTracker: Send + Sync {
    /// Start tracking a deployment
    ///
    /// # Errors
    /// Returns error if tracking file creation fails
    fn start_tracking(
        &self,
        deployment_id: &str,
        host_command_identifier: &str,
    ) -> Result<(), DeploymentTrackerError>;

    /// Stop tracking a deployment
    ///
    /// # Errors
    /// Returns error if tracking file deletion fails
    fn stop_tracking(&self, deployment_id: &str) -> Result<(), DeploymentTrackerError>;

    /// Get the currently active deployment, if any
    ///
    /// # Errors
    /// Returns error if reading tracking files fails
    fn get_active_deployment(&self) -> Result<Option<ActiveDeployment>, DeploymentTrackerError>;

    /// Check if any deployment is currently in progress
    ///
    /// # Errors
    /// Returns error if checking tracking files fails
    fn is_deployment_in_progress(&self) -> Result<bool, DeploymentTrackerError>;

    /// Clean all tracking data
    ///
    /// # Errors
    /// Returns error if cleanup fails
    fn clean_all(&self) -> Result<(), DeploymentTrackerError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_deployment_is_cloneable() {
        let ad = ActiveDeployment {
            deployment_id: "d-1".into(),
            host_command_identifier: "cmd-1".into(),
            timestamp: 12345,
        };
        let cloned = ad.clone();
        assert_eq!(cloned.deployment_id, "d-1");
        assert_eq!(cloned.host_command_identifier, "cmd-1");
        assert_eq!(cloned.timestamp, 12345);
    }

    #[test]
    fn active_deployment_is_debuggable() {
        let ad = ActiveDeployment {
            deployment_id: "d-42".into(),
            host_command_identifier: "cmd-99".into(),
            timestamp: 0,
        };
        let debug = format!("{ad:?}");
        assert!(debug.contains("d-42"));
        assert!(debug.contains("cmd-99"));
    }

    #[test]
    fn tracker_error_from_io() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "gone");
        let err = DeploymentTrackerError::from(io_err);
        assert!(err.to_string().contains("gone"));
    }

    #[test]
    fn tracker_error_invalid_id_display() {
        let err = DeploymentTrackerError::InvalidDeploymentId("bad-id".into());
        assert_eq!(err.to_string(), "Invalid deployment ID: bad-id");
    }

    #[test]
    fn tracker_error_io_display() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err = DeploymentTrackerError::Io(io_err);
        assert!(err.to_string().contains("denied"));
    }
}
