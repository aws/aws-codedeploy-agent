//! @risk none
//!
//! Deployment specification types — `DeploymentSpec`, `RevisionLocation`, `Envelope`.
use serde::{Deserialize, Serialize};

pub const DEFAULT_FILE_EXISTS_BEHAVIOR: &str = "DISALLOW";
pub const DEFAULT_APP_SPEC_PATH: &str = "appspec.yml";
pub const DEFAULT_DEPLOYMENT_CREATOR: &str = "user";
pub const DEFAULT_DEPLOYMENT_TYPE: &str = "IN_PLACE";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RevisionSource {
    S3,
    GitHub,
    #[serde(rename = "Local File")]
    LocalFile,
    #[serde(rename = "Local Directory")]
    LocalDirectory,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RevisionLocation {
    S3 {
        bucket: String,
        key: String,
        bundle_type: String,
        version: Option<String>,
        etag: Option<String>,
    },
    GitHub {
        account: String,
        repository: String,
        commit_id: String,
        #[serde(skip)]
        anonymous: bool,
        #[serde(skip)]
        auth_token: Option<String>,
        bundle_type: Option<String>,
    },
    Local {
        location: String,
        bundle_type: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentSpec {
    pub deployment_id: String,
    pub deployment_group_id: String,
    pub deployment_group_name: String,
    pub application_name: String,
    pub deployment_creator: String,
    pub deployment_type: String,
    pub app_spec_path: String,
    pub file_exists_behavior: String,
    pub revision_source: RevisionSource,
    pub revision: RevisionLocation,
    pub all_possible_lifecycle_events: Option<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct Envelope {
    pub format: String,
    pub payload: String,
}
