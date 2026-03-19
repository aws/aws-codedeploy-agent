//! @risk high
//!
//! Deployment spec builder — validates required fields and constructs `DeploymentSpec`.
//!
//! Extracts deployment metadata (ID, group, app name, creator, type) from the
//! parsed JSON, applies defaults for optional fields, and resolves ARN-format
//! deployment IDs to their short form.

use super::error::{DeploymentSpecError, Result};
use super::types::{
    DEFAULT_APP_SPEC_PATH, DEFAULT_DEPLOYMENT_CREATOR, DEFAULT_DEPLOYMENT_TYPE,
    DEFAULT_FILE_EXISTS_BEHAVIOR, DeploymentSpec, RevisionLocation, RevisionSource,
};
use serde_json::Value;

// Check if a JSON property is present, non-null, and non-empty.
// Checks: has_key? && !nil? && !empty?
// Works on both strings and arrays.
fn property_set(obj: &Value, key: &str) -> bool {
    obj.get(key)
        .and_then(|v| {
            if v.is_null() {
                None
            } else if let Some(s) = v.as_str() {
                if s.is_empty() { None } else { Some(()) }
            } else if let Some(arr) = v.as_array() {
                if arr.is_empty() { None } else { Some(()) }
            } else {
                Some(())
            }
        })
        .is_some()
}

fn extract_deployment_id(arn: &str) -> String {
    if arn.starts_with("arn:") {
        arn.split(':')
            .nth(5)
            .and_then(|s| s.split('/').nth(1))
            .unwrap_or(arn)
            .to_string()
    } else {
        arn.to_string()
    }
}

pub(super) fn build(
    data: &Value,
    revision_source: RevisionSource,
    revision: RevisionLocation,
) -> Result<DeploymentSpec> {
    // Validate required fields
    if !property_set(data, "DeploymentId") {
        return Err(DeploymentSpecError::MissingField("DeploymentId".to_string()));
    }
    if !property_set(data, "DeploymentGroupId") {
        return Err(DeploymentSpecError::MissingField("DeploymentGroupId".to_string()));
    }
    if !property_set(data, "DeploymentGroupName") {
        return Err(DeploymentSpecError::MissingField("DeploymentGroupName".to_string()));
    }
    if !property_set(data, "ApplicationName") {
        return Err(DeploymentSpecError::MissingField("ApplicationName".to_string()));
    }

    let application_name = data["ApplicationName"].as_str().unwrap().to_string();
    let deployment_group_name = data["DeploymentGroupName"].as_str().unwrap().to_string();
    let deployment_id = extract_deployment_id(data["DeploymentId"].as_str().unwrap());
    let deployment_group_id = data["DeploymentGroupId"].as_str().unwrap().to_string();

    let deployment_creator = data
        .get("DeploymentCreator")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_DEPLOYMENT_CREATOR)
        .to_string();

    let deployment_type = data
        .get("DeploymentType")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_DEPLOYMENT_TYPE)
        .to_string();

    let app_spec_path = data
        .get("AppSpecFilename")
        .and_then(|v| v.as_str())
        .unwrap_or(DEFAULT_APP_SPEC_PATH)
        .to_string();

    let file_exists_behavior = data
        .get("AgentActionOverrides")
        .and_then(|overrides| overrides.get("AgentOverrides"))
        .and_then(|agent_overrides| agent_overrides.get("FileExistsBehavior"))
        .and_then(|v| v.as_str())
        .map_or_else(|| DEFAULT_FILE_EXISTS_BEHAVIOR.to_string(), str::to_uppercase);

    let all_possible_lifecycle_events =
        data.get("AllPossibleLifecycleEvents").and_then(|v| v.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|v: &Value| v.as_str().map(String::from))
                .collect::<Vec<String>>()
        });

    Ok(DeploymentSpec {
        deployment_id,
        deployment_group_id,
        deployment_group_name,
        application_name,
        deployment_creator,
        deployment_type,
        app_spec_path,
        file_exists_behavior,
        revision_source,
        revision,
        all_possible_lifecycle_events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
    use serde_json::json;

    #[test]
    fn build_minimal() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let spec = build(&data, revision_source, revision).unwrap();
        assert_eq!(spec.deployment_id, "d-12345678");
        assert_eq!(spec.deployment_group_id, "dg-12345678");
        assert_eq!(spec.deployment_group_name, "MyGroup");
        assert_eq!(spec.application_name, "MyApp");
        assert_eq!(spec.deployment_creator, "user");
        assert_eq!(spec.deployment_type, "IN_PLACE");
        assert_eq!(spec.app_spec_path, "appspec.yml");
        assert_eq!(spec.file_exists_behavior, "DISALLOW");
        assert_eq!(spec.all_possible_lifecycle_events, None);
    }

    #[test]
    fn build_with_custom_values() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp",
            "DeploymentCreator": "autoscaling",
            "DeploymentType": "BLUE_GREEN",
            "AppSpecFilename": "custom.yml"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let spec = build(&data, revision_source, revision).unwrap();
        assert_eq!(spec.deployment_creator, "autoscaling");
        assert_eq!(spec.deployment_type, "BLUE_GREEN");
        assert_eq!(spec.app_spec_path, "custom.yml");
    }

    #[test]
    fn build_with_file_exists_behavior() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp",
            "AgentActionOverrides": {
                "AgentOverrides": {
                    "FileExistsBehavior": "overwrite"
                }
            }
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let spec = build(&data, revision_source, revision).unwrap();
        assert_eq!(spec.file_exists_behavior, "OVERWRITE");
    }

    #[test]
    fn build_with_lifecycle_events() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp",
            "AllPossibleLifecycleEvents": ["BeforeInstall", "AfterInstall"]
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let spec = build(&data, revision_source, revision).unwrap();
        assert_eq!(
            spec.all_possible_lifecycle_events,
            Some(vec!["BeforeInstall".to_string(), "AfterInstall".to_string()])
        );
    }

    #[test]
    fn build_with_lifecycle_events_mixed_types() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp",
            "AllPossibleLifecycleEvents": ["BeforeInstall", 123, "AfterInstall", null]
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let spec = build(&data, revision_source, revision).unwrap();
        // Only strings are kept
        assert_eq!(
            spec.all_possible_lifecycle_events,
            Some(vec!["BeforeInstall".to_string(), "AfterInstall".to_string()])
        );
    }

    #[test]
    fn build_with_arn_deployment_id() {
        let data = json!({
            "DeploymentId": "arn:aws:codedeploy:us-east-1:123456789012:deployment/d-ABCD1234",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let spec = build(&data, revision_source, revision).unwrap();
        assert_eq!(spec.deployment_id, "d-ABCD1234");
    }

    #[test]
    fn build_missing_deployment_id() {
        let data = json!({
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let result = build(&data, revision_source, revision);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("DeploymentId"));
    }

    #[test]
    fn build_missing_deployment_group_id() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let result = build(&data, revision_source, revision);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("DeploymentGroupId"));
    }

    #[test]
    fn build_missing_deployment_group_name() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let result = build(&data, revision_source, revision);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("DeploymentGroupName"));
    }

    #[test]
    fn build_missing_application_name() {
        let data = json!({
            "DeploymentId": "d-12345678",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let result = build(&data, revision_source, revision);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("ApplicationName"));
    }

    #[test]
    fn build_null_deployment_id() {
        let data = json!({
            "DeploymentId": null,
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let result = build(&data, revision_source, revision);
        assert!(result.is_err());
    }

    #[test]
    fn build_empty_deployment_id() {
        let data = json!({
            "DeploymentId": "",
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let result = build(&data, revision_source, revision);
        assert!(result.is_err());
    }

    #[test]
    fn build_deployment_id_as_array() {
        let data = json!({
            "DeploymentId": [],
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let result = build(&data, revision_source, revision);
        assert!(result.is_err());
    }

    #[test]
    fn build_deployment_id_as_number() {
        let data = json!({
            "DeploymentId": 123,
            "DeploymentGroupId": "dg-12345678",
            "DeploymentGroupName": "MyGroup",
            "ApplicationName": "MyApp"
        });

        let revision_source = RevisionSource::S3;
        let revision = RevisionLocation::S3 {
            bucket: "bucket".to_string(),
            key: "key".to_string(),
            bundle_type: "tar".to_string(),
            version: None,
            etag: None,
        };

        let result = std::panic::catch_unwind(|| build(&data, revision_source, revision));
        assert!(result.is_err());
    }
}
