//! @risk medium
//!
//! Revision source parsing — dispatches to S3, GitHub, or Local parsers
//! based on the `RevisionType` field in the deployment specification.

mod github;
mod local;
mod s3;

use crate::deployment_specification::error::{DeploymentSpecError, Result};
use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
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

pub(super) fn parse(data: &Value) -> Result<(RevisionSource, RevisionLocation)> {
    if !property_set(data, "Revision") {
        return Err(DeploymentSpecError::MissingField("Must specify a revison".to_string()));
    }

    let revision_data = &data["Revision"];
    if !property_set(revision_data, "RevisionType") {
        return Err(DeploymentSpecError::MissingField(
            "Must specify a revision source".to_string(),
        ));
    }

    let revision_type = revision_data["RevisionType"].as_str().unwrap();

    match revision_type {
        "S3" => {
            let s3_rev = &revision_data["S3Revision"];
            s3::parse(s3_rev)
        },
        "GitHub" => {
            let gh_rev = &revision_data["GitHubRevision"];
            let auth_token =
                data.get("GitHubAccessToken").and_then(|v| v.as_str()).map(String::from);
            github::parse(gh_rev, auth_token)
        },
        "Local File" | "Local Directory" => {
            let local_rev = &revision_data["LocalRevision"];
            local::parse(local_rev, revision_type)
        },
        _ => Err(DeploymentSpecError::InvalidRevision(
            "Exactly one of S3Revision, GitHubRevision, or LocalRevision must be specified"
                .to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
    use serde_json::json;

    #[test]
    fn parse_s3_revision() {
        let data = json!({
            "Revision": {
                "RevisionType": "S3",
                "S3Revision": {
                    "Bucket": "my-bucket",
                    "Key": "my-key.tar.gz",
                    "BundleType": "tar"
                }
            }
        });

        let result = parse(&data).unwrap();
        assert_eq!(result.0, RevisionSource::S3);

        assert_eq!(
            result.1,
            RevisionLocation::S3 {
                bucket: "my-bucket".to_string(),
                key: "my-key.tar.gz".to_string(),
                bundle_type: "tar".to_string(),
                version: None,
                etag: None,
            }
        );
    }

    #[test]
    fn parse_github_revision_with_token() {
        let data = json!({
            "Revision": {
                "RevisionType": "GitHub",
                "GitHubRevision": {
                    "Account": "my-account",
                    "Repository": "my-repo",
                    "CommitId": "abc123"
                }
            },
            "GitHubAccessToken": "token123"
        });

        let result = parse(&data).unwrap();
        assert_eq!(result.0, RevisionSource::GitHub);

        assert_eq!(
            result.1,
            RevisionLocation::GitHub {
                account: "my-account".to_string(),
                repository: "my-repo".to_string(),
                commit_id: "abc123".to_string(),
                anonymous: false,
                auth_token: Some("token123".to_string()),
                bundle_type: None,
            }
        );
    }

    #[test]
    fn parse_github_revision_without_token() {
        let data = json!({
            "Revision": {
                "RevisionType": "GitHub",
                "GitHubRevision": {
                    "Account": "my-account",
                    "Repository": "my-repo",
                    "CommitId": "abc123"
                }
            }
        });

        let result = parse(&data).unwrap();

        assert_eq!(
            result.1,
            RevisionLocation::GitHub {
                account: "my-account".to_string(),
                repository: "my-repo".to_string(),
                commit_id: "abc123".to_string(),
                anonymous: true,
                auth_token: None,
                bundle_type: None,
            }
        );
    }

    #[test]
    fn parse_local_file_revision() {
        let data = json!({
            "Revision": {
                "RevisionType": "Local File",
                "LocalRevision": {
                    "Location": "/tmp/app.tar",
                    "BundleType": "tar"
                }
            }
        });

        let result = parse(&data).unwrap();
        assert_eq!(result.0, RevisionSource::LocalFile);

        assert_eq!(
            result.1,
            RevisionLocation::Local {
                location: "/tmp/app.tar".to_string(),
                bundle_type: "tar".to_string(),
            }
        );
    }

    #[test]
    fn parse_local_directory_revision() {
        let data = json!({
            "Revision": {
                "RevisionType": "Local Directory",
                "LocalRevision": {
                    "Location": "/tmp/app-dir",
                    "BundleType": "directory"
                }
            }
        });

        let result = parse(&data).unwrap();
        assert_eq!(result.0, RevisionSource::LocalDirectory);
    }

    #[test]
    fn parse_missing_revision() {
        let data = json!({});

        let result = parse(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("revison"));
    }

    #[test]
    fn parse_null_revision() {
        let data = json!({
            "Revision": null
        });

        let result = parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_revision() {
        let data = json!({
            "Revision": {}
        });

        let result = parse(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("revision source"));
    }

    #[test]
    fn parse_missing_revision_type() {
        let data = json!({
            "Revision": {
                "S3Revision": {
                    "Bucket": "my-bucket",
                    "Key": "my-key.tar.gz",
                    "BundleType": "tar"
                }
            }
        });

        let result = parse(&data);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("revision source"));
    }

    #[test]
    fn parse_null_revision_type() {
        let data = json!({
            "Revision": {
                "RevisionType": null
            }
        });

        let result = parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_revision_type() {
        let data = json!({
            "Revision": {
                "RevisionType": ""
            }
        });

        let result = parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn parse_invalid_revision_type() {
        let data = json!({
            "Revision": {
                "RevisionType": "Unknown"
            }
        });

        let result = parse(&data);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Exactly one of S3Revision, GitHubRevision, or LocalRevision")
        );
    }

    #[test]
    fn parse_revision_as_array_empty() {
        let data = json!({
            "Revision": []
        });

        let result = parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn parse_revision_as_array_nonempty() {
        let data = json!({
            "Revision": [{"RevisionType": "S3"}]
        });

        // Non-empty array passes first property_set, but fails when checking RevisionType
        let result = parse(&data);
        assert!(result.is_err());
    }

    #[test]
    fn parse_revision_as_number() {
        let data = json!({
            "Revision": 123
        });

        // Number passes first property_set, but fails when checking RevisionType
        let result = parse(&data);
        assert!(result.is_err());
    }
}
