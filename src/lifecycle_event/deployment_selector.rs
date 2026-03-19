//! @risk medium
//!
//! Deployment directory selection logic

use crate::lifecycle_event::{DeploymentType, LifecycleEventType};
use std::path::{Path, PathBuf};

const LAST_SUCCESSFUL: &str = "LastSuccessful";
const MOST_RECENT: &str = "MostRecent";
const CURRENT: &str = "Current";

/// Select the correct deployment directory based on lifecycle event, deployment creator, and type
pub fn select_deployment_dir(
    lifecycle_event: LifecycleEventType,
    deployment_creator: &str,
    deployment_type: DeploymentType,
    current_dir: &Path,
    last_successful_dir: Option<&Path>,
    most_recent_dir: Option<&Path>,
) -> PathBuf {
    let mapping = if deployment_creator == "codeDeployRollback"
        && deployment_type == DeploymentType::BlueGreen
    {
        rollback_mapping(lifecycle_event)
    } else {
        standard_mapping(lifecycle_event)
    };

    let deployment_archive = current_dir.join("deployment-archive");

    match mapping {
        LAST_SUCCESSFUL if !deployment_archive.exists() => {
            last_successful_dir.map_or_else(|| current_dir.to_path_buf(), Path::to_path_buf)
        },
        MOST_RECENT if !deployment_archive.exists() => {
            most_recent_dir.map_or_else(|| current_dir.to_path_buf(), Path::to_path_buf)
        },
        _ => current_dir.to_path_buf(),
    }
}

fn standard_mapping(event: LifecycleEventType) -> &'static str {
    match event {
        LifecycleEventType::BeforeBlockTraffic
        | LifecycleEventType::AfterBlockTraffic
        | LifecycleEventType::ApplicationStop => LAST_SUCCESSFUL,
        LifecycleEventType::BeforeInstall
        | LifecycleEventType::AfterInstall
        | LifecycleEventType::ApplicationStart
        | LifecycleEventType::BeforeAllowTraffic
        | LifecycleEventType::AfterAllowTraffic
        | LifecycleEventType::ValidateService => CURRENT,
    }
}

fn rollback_mapping(event: LifecycleEventType) -> &'static str {
    match event {
        LifecycleEventType::BeforeBlockTraffic | LifecycleEventType::AfterBlockTraffic => {
            MOST_RECENT
        },
        LifecycleEventType::ApplicationStop
        | LifecycleEventType::BeforeAllowTraffic
        | LifecycleEventType::AfterAllowTraffic => LAST_SUCCESSFUL,
        LifecycleEventType::BeforeInstall
        | LifecycleEventType::AfterInstall
        | LifecycleEventType::ApplicationStart
        | LifecycleEventType::ValidateService => CURRENT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn standard_before_block_traffic() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let last_successful = temp.path().join("last_successful");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&last_successful).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::BeforeBlockTraffic,
            "user",
            DeploymentType::InPlace,
            &current,
            Some(&last_successful),
            None,
        );
        assert_eq!(result, last_successful);
    }

    #[test]
    fn standard_after_block_traffic() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let last_successful = temp.path().join("last_successful");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&last_successful).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::AfterBlockTraffic,
            "user",
            DeploymentType::InPlace,
            &current,
            Some(&last_successful),
            None,
        );
        assert_eq!(result, last_successful);
    }

    #[test]
    fn standard_application_stop() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let last_successful = temp.path().join("last_successful");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&last_successful).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::ApplicationStop,
            "user",
            DeploymentType::InPlace,
            &current,
            Some(&last_successful),
            None,
        );
        assert_eq!(result, last_successful);
    }

    #[test]
    fn standard_before_install() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::BeforeInstall,
            "user",
            DeploymentType::InPlace,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn standard_after_install() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::AfterInstall,
            "user",
            DeploymentType::InPlace,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn standard_application_start() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::ApplicationStart,
            "user",
            DeploymentType::InPlace,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn standard_before_allow_traffic() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::BeforeAllowTraffic,
            "user",
            DeploymentType::InPlace,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn standard_after_allow_traffic() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::AfterAllowTraffic,
            "user",
            DeploymentType::InPlace,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn standard_validate_service() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::ValidateService,
            "user",
            DeploymentType::InPlace,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn rollback_before_block_traffic() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let most_recent = temp.path().join("most_recent");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&most_recent).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::BeforeBlockTraffic,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            None,
            Some(&most_recent),
        );
        assert_eq!(result, most_recent);
    }

    #[test]
    fn rollback_after_block_traffic() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let most_recent = temp.path().join("most_recent");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&most_recent).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::AfterBlockTraffic,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            None,
            Some(&most_recent),
        );
        assert_eq!(result, most_recent);
    }

    #[test]
    fn rollback_application_stop() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let last_successful = temp.path().join("last_successful");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&last_successful).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::ApplicationStop,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            Some(&last_successful),
            None,
        );
        assert_eq!(result, last_successful);
    }

    #[test]
    fn rollback_before_install() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::BeforeInstall,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn rollback_after_install() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::AfterInstall,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn rollback_application_start() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::ApplicationStart,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn rollback_before_allow_traffic() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let last_successful = temp.path().join("last_successful");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&last_successful).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::BeforeAllowTraffic,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            Some(&last_successful),
            None,
        );
        assert_eq!(result, last_successful);
    }

    #[test]
    fn rollback_after_allow_traffic() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let last_successful = temp.path().join("last_successful");
        fs::create_dir_all(&current).unwrap();
        fs::create_dir_all(&last_successful).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::AfterAllowTraffic,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            Some(&last_successful),
            None,
        );
        assert_eq!(result, last_successful);
    }

    #[test]
    fn rollback_validate_service() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        fs::create_dir_all(&current).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::ValidateService,
            "codeDeployRollback",
            DeploymentType::BlueGreen,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }

    #[test]
    fn with_archive_exists() {
        let temp = TempDir::new().unwrap();
        let current = temp.path().join("current");
        let archive = current.join("deployment-archive");
        fs::create_dir_all(&archive).unwrap();

        let result = select_deployment_dir(
            LifecycleEventType::ApplicationStop,
            "user",
            DeploymentType::InPlace,
            &current,
            None,
            None,
        );
        assert_eq!(result, current);
    }
}
