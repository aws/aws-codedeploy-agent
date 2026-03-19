//! @risk medium
//!
//! GitHub revision parsing.
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
    for field in &["Account", "Repository", "CommitId"] {
        if !property_set(revision, field) {
            return Err(DeploymentSpecError::InvalidRevision(
                "GitHubRevision in Deployment Spec must specify Account, Repository and CommitId"
                    .to_string(),
            ));
        }
    }

    // If Anonymous is false, OAuthToken is required
    if let Some(anonymous) = revision.get("Anonymous").and_then(serde_json::Value::as_bool)
        && !anonymous
        && !property_set(revision, "OAuthToken")
    {
        return Err(DeploymentSpecError::InvalidRevision(
            "GitHubRevision with Anonymous=false must specify OAuthToken".to_string(),
        ));
    }

    Ok(())
}

pub(super) fn parse(
    gh_rev: &Value,
    auth_token: Option<String>,
) -> Result<(RevisionSource, RevisionLocation)> {
    validate(gh_rev)?;

    let anonymous = auth_token.is_none();

    Ok((
        RevisionSource::GitHub,
        RevisionLocation::GitHub {
            account: gh_rev["Account"].as_str().unwrap().to_string(),
            repository: gh_rev["Repository"].as_str().unwrap().to_string(),
            commit_id: gh_rev["CommitId"].as_str().unwrap().to_string(),
            anonymous,
            auth_token,
            bundle_type: gh_rev.get("BundleType").and_then(|v| v.as_str()).map(String::from),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::{RevisionLocation, RevisionSource};
    use serde_json::json;

    #[test]
    fn parse_valid_minimal_with_token() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": "abc123def456"
        });

        let result = parse(&gh_rev, Some("token123".to_string())).unwrap();
        assert_eq!(result.0, RevisionSource::GitHub);

        assert_eq!(
            result.1,
            RevisionLocation::GitHub {
                account: "my-account".to_string(),
                repository: "my-repo".to_string(),
                commit_id: "abc123def456".to_string(),
                anonymous: false,
                auth_token: Some("token123".to_string()),
                bundle_type: None,
            }
        );
    }

    #[test]
    fn parse_valid_minimal_without_token() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": "abc123def456"
        });

        let result = parse(&gh_rev, None).unwrap();

        assert_eq!(
            result.1,
            RevisionLocation::GitHub {
                account: "my-account".to_string(),
                repository: "my-repo".to_string(),
                commit_id: "abc123def456".to_string(),
                anonymous: true,
                auth_token: None,
                bundle_type: None,
            }
        );
    }

    #[test]
    fn parse_with_bundle_type() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": "abc123def456",
            "BundleType": "zip"
        });

        let result = parse(&gh_rev, None).unwrap();

        assert_eq!(
            result.1,
            RevisionLocation::GitHub {
                account: "my-account".to_string(),
                repository: "my-repo".to_string(),
                commit_id: "abc123def456".to_string(),
                anonymous: true,
                auth_token: None,
                bundle_type: Some("zip".to_string()),
            }
        );
    }

    #[test]
    fn parse_missing_account() {
        let gh_rev = json!({
            "Repository": "my-repo",
            "CommitId": "abc123def456"
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Account, Repository and CommitId"));
    }

    #[test]
    fn parse_missing_repository() {
        let gh_rev = json!({
            "Account": "my-account",
            "CommitId": "abc123def456"
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Account, Repository and CommitId"));
    }

    #[test]
    fn parse_missing_commit_id() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo"
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Account, Repository and CommitId"));
    }

    #[test]
    fn parse_null_account() {
        let gh_rev = json!({
            "Account": null,
            "Repository": "my-repo",
            "CommitId": "abc123def456"
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_account() {
        let gh_rev = json!({
            "Account": "",
            "Repository": "my-repo",
            "CommitId": "abc123def456"
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_repository() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "",
            "CommitId": "abc123def456"
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
    }

    #[test]
    fn parse_empty_commit_id() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": ""
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
    }

    #[test]
    fn parse_anonymous_false_with_oauth_token() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": "abc123def456",
            "Anonymous": false,
            "OAuthToken": "token123"
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_anonymous_false_without_oauth_token() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": "abc123def456",
            "Anonymous": false
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Anonymous=false must specify OAuthToken")
        );
    }

    #[test]
    fn parse_anonymous_true_without_oauth_token() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": "abc123def456",
            "Anonymous": true
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_ok());
    }

    #[test]
    fn parse_null_bundle_type() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": "abc123def456",
            "BundleType": null
        });

        let result = parse(&gh_rev, None).unwrap();

        assert_eq!(
            result.1,
            RevisionLocation::GitHub {
                account: "my-account".to_string(),
                repository: "my-repo".to_string(),
                commit_id: "abc123def456".to_string(),
                anonymous: true,
                auth_token: None,
                bundle_type: None,
            }
        );
    }

    #[test]
    fn parse_empty_bundle_type() {
        let gh_rev = json!({
            "Account": "my-account",
            "Repository": "my-repo",
            "CommitId": "abc123def456",
            "BundleType": ""
        });

        let result = parse(&gh_rev, None).unwrap();

        assert_eq!(
            result.1,
            RevisionLocation::GitHub {
                account: "my-account".to_string(),
                repository: "my-repo".to_string(),
                commit_id: "abc123def456".to_string(),
                anonymous: true,
                auth_token: None,
                bundle_type: Some(String::new()),
            }
        );
    }

    #[test]
    fn parse_account_as_array_empty() {
        let gh_rev = json!({
            "Account": [],
            "Repository": "my-repo",
            "CommitId": "abc123def456"
        });

        let result = parse(&gh_rev, None);
        assert!(result.is_err());
    }

    #[test]
    fn parse_account_as_array_nonempty() {
        let gh_rev = json!({
            "Account": ["my-account"],
            "Repository": "my-repo",
            "CommitId": "abc123def456"
        });

        let result = std::panic::catch_unwind(|| parse(&gh_rev, None));
        assert!(result.is_err());
    }

    #[test]
    fn parse_account_as_number() {
        let gh_rev = json!({
            "Account": 123,
            "Repository": "my-repo",
            "CommitId": "abc123def456"
        });

        let result = std::panic::catch_unwind(|| parse(&gh_rev, None));
        assert!(result.is_err());
    }
}
