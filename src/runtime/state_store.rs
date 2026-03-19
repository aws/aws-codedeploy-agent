//! @risk low
//!
//! Persistent state store for agent metadata.
use serde::{Deserialize, Serialize};

use super::error::RuntimeError;

/// Checkpoint data for a deployment
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub deployment_id: String,
    pub command_id: String,
    pub state_data: Vec<u8>,
    pub timestamp: u64,
}

/// Deployment history entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentHistory {
    pub deployment_id: String,
    pub command_id: String,
    pub status: String,
    pub start_time: u64,
    pub end_time: Option<u64>,
}

/// Trait for storing and retrieving deployment state
pub trait StateStore: Send + Sync {
    /// Save a checkpoint for a deployment
    /// # Errors
    /// Returns an error if saving fails.
    fn save_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), RuntimeError>;

    /// Load a checkpoint for a deployment
    /// # Errors
    /// Returns an error if loading fails.
    fn load_checkpoint(&self, deployment_id: &str) -> Result<Option<Checkpoint>, RuntimeError>;

    /// Delete a checkpoint for a deployment
    /// # Errors
    /// Returns an error if deletion fails.
    fn delete_checkpoint(&self, deployment_id: &str) -> Result<(), RuntimeError>;

    /// List all checkpoints
    /// # Errors
    /// Returns an error if listing fails.
    fn list_checkpoints(&self) -> Result<Vec<String>, RuntimeError>;

    /// Save deployment history
    /// # Errors
    /// Returns an error if saving fails.
    fn save_history(&self, history: &DeploymentHistory) -> Result<(), RuntimeError>;

    /// Get deployment history
    /// # Errors
    /// Returns an error if fetching fails.
    fn get_history(&self, deployment_id: &str) -> Result<Option<DeploymentHistory>, RuntimeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_is_cloneable() {
        let cp = Checkpoint {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            state_data: vec![1, 2, 3],
            timestamp: 1000,
        };
        let cloned = cp.clone();
        assert_eq!(cloned.deployment_id, "d-1");
        assert_eq!(cloned.state_data, vec![1, 2, 3]);
        assert_eq!(cloned.timestamp, 1000);
    }

    #[test]
    fn checkpoint_is_debuggable() {
        let cp = Checkpoint {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            state_data: vec![],
            timestamp: 0,
        };
        let debug = format!("{cp:?}");
        assert!(debug.contains("d-1"));
    }

    #[test]
    fn checkpoint_serializes_to_json() {
        let cp = Checkpoint {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            state_data: vec![10, 20],
            timestamp: 999,
        };
        let json = serde_json::to_string(&cp).unwrap();
        assert!(json.contains("d-1"));
        assert!(json.contains("999"));
    }

    #[test]
    fn checkpoint_deserializes_from_json() {
        let json = r#"{"deployment_id":"d-2","command_id":"c-2","state_data":[5],"timestamp":42}"#;
        let cp: Checkpoint = serde_json::from_str(json).unwrap();
        assert_eq!(cp.deployment_id, "d-2");
        assert_eq!(cp.state_data, vec![5]);
        assert_eq!(cp.timestamp, 42);
    }

    #[test]
    fn deployment_history_is_cloneable() {
        let h = DeploymentHistory {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            status: "Succeeded".into(),
            start_time: 100,
            end_time: Some(200),
        };
        let cloned = h.clone();
        assert_eq!(cloned.status, "Succeeded");
        assert_eq!(cloned.end_time, Some(200));
    }

    #[test]
    fn deployment_history_none_end_time() {
        let h = DeploymentHistory {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            status: "InProgress".into(),
            start_time: 100,
            end_time: None,
        };
        assert!(h.end_time.is_none());
    }

    #[test]
    fn deployment_history_serializes_to_json() {
        let h = DeploymentHistory {
            deployment_id: "d-1".into(),
            command_id: "c-1".into(),
            status: "Failed".into(),
            start_time: 50,
            end_time: None,
        };
        let json = serde_json::to_string(&h).unwrap();
        assert!(json.contains("Failed"));
        assert!(json.contains("null"));
    }

    #[test]
    fn deployment_history_deserializes_from_json() {
        let json = r#"{"deployment_id":"d-3","command_id":"c-3","status":"Succeeded","start_time":1,"end_time":2}"#;
        let h: DeploymentHistory = serde_json::from_str(json).unwrap();
        assert_eq!(h.deployment_id, "d-3");
        assert_eq!(h.end_time, Some(2));
    }
}
