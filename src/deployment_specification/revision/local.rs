//! @risk medium
//!
//! Local file/directory revision parsing.
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

fn validate(revision: &Value) -> Result<()> {
    for field in &["Location", "BundleType"] {
        if !property_set(revision, field) {
            return Err(DeploymentSpecError::InvalidRevision(
                "LocalRevision in Deployment Spec must specify Location and BundleType".to_string(),
            ));
        }
    }

    let bundle_type = revision["BundleType"].as_str().unwrap();
    if !["tar", "tgz", "zip", "directory"].contains(&bundle_type) {
        return Err(DeploymentSpecError::InvalidBundleType(
            "BundleType in LocalRevision must be tar, tgz, zip, or directory".to_string(),
        ));
    }

    Ok(())
}

pub(super) fn parse(
    local_rev: &Value,
    revision_type: &str,
) -> Result<(RevisionSource, RevisionLocation)> {
    validate(local_rev)?;

    let source = if revision_type == "Local File" {
        RevisionSource::LocalFile
    } else {
        RevisionSource::LocalDirectory
    };

    Ok((
        source,
        RevisionLocation::Local {
            location: local_rev["Location"].as_str().unwrap().to_string(),
            bundle_type: local_rev["BundleType"].as_str().unwrap().to_string(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
    use serde_json::json;

    #[test]
    fn parse_local_file_tar() {
        let local_rev = json!({
            "Location": "/tmp/app.tar",
            "BundleType": "tar"
        });

        let result = parse(&local_rev, "Local File").unwrap();
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
    fn parse_local_file_tgz() {
        let local_rev = json!({
            "Location": "/tmp/app.tgz",
            "BundleType": "tgz"
        });

        let result = parse(&local_rev, "Local File").unwrap();
        assert_eq!(
            result.1,
            RevisionLocation::Local {
                location: "/tmp/app.tgz".to_string(),
                bundle_type: "tgz".to_string(),
            }
        );
    }

    #[test]
    fn parse_local_file_zip() {
        let local_rev = json!({
            "Location": "/tmp/app.zip",
            "BundleType": "zip"
        });

        let result = parse(&local_rev, "Local File").unwrap();
        assert_eq!(
            result.1,
            RevisionLocation::Local {
                location: "/tmp/app.zip".to_string(),
                bundle_type: "zip".to_string(),
            }
        );
    }

    #[test]
    fn parse_local_directory() {
        let local_rev = json!({
            "Location": "/tmp/app-dir",
            "BundleType": "directory"
        });

        let result = parse(&local_rev, "Local Directory").unwrap();
        assert_eq!(result.0, RevisionSource::LocalDirectory);

        assert_eq!(
            result.1,
            RevisionLocation::Local {
                location: "/tmp/app-dir".to_string(),
                bundle_type: "directory".to_string(),
            }
        );
    }

    #[test]
    fn parse_missing_location() {
        let local_rev = json!({
            "BundleType": "tar"
        });

        let result = parse(&local_rev, "Local File");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Location and BundleType"));
    }

    #[test]
    fn parse_missing_bundle_type() {
        let local_rev = json!({
            "Location": "/tmp/app.tar"
        });

        let result = parse(&local_rev, "Local File");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Location and BundleType"));
    }

    #[test]
    fn parse_null_location() {
        let local_rev = json!({
            "Location": null,
            "BundleType": "tar"
        });

        let result = parse(&local_rev, "Local File");
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_location() {
        let local_rev = json!({
            "Location": "",
            "BundleType": "tar"
        });

        let result = parse(&local_rev, "Local File");
        assert!(result.is_err());
    }

    #[test]
    fn parse_null_bundle_type() {
        let local_rev = json!({
            "Location": "/tmp/app.tar",
            "BundleType": null
        });

        let result = parse(&local_rev, "Local File");
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_bundle_type() {
        let local_rev = json!({
            "Location": "/tmp/app.tar",
            "BundleType": ""
        });

        let result = parse(&local_rev, "Local File");
        assert!(result.is_err());
    }

    #[test]
    fn parse_invalid_bundle_type() {
        let local_rev = json!({
            "Location": "/tmp/app.tar",
            "BundleType": "invalid"
        });

        let result = parse(&local_rev, "Local File");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("tar, tgz, zip, or directory"));
    }

    #[test]
    fn parse_location_as_array_empty() {
        let local_rev = json!({
            "Location": [],
            "BundleType": "tar"
        });

        let result = parse(&local_rev, "Local File");
        assert!(result.is_err());
    }

    #[test]
    fn parse_location_as_array_nonempty() {
        let local_rev = json!({
            "Location": ["/tmp/app.tar"],
            "BundleType": "tar"
        });

        let result = std::panic::catch_unwind(|| parse(&local_rev, "Local File"));
        assert!(result.is_err());
    }

    #[test]
    fn parse_location_as_number() {
        let local_rev = json!({
            "Location": 123,
            "BundleType": "tar"
        });

        let result = std::panic::catch_unwind(|| parse(&local_rev, "Local File"));
        assert!(result.is_err());
    }
}
