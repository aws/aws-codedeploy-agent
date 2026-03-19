//! @risk high
//!
//! Deployment specification parsing and verification.
//!
//! Handles the full pipeline from raw envelope to typed `DeploymentSpec`:
//! 1. `envelope` — verifies PKCS7 signature (or TEXT/JSON in developer mode)
//! 2. `parse` — deserializes JSON, redacts secrets, builds the spec
//! 3. `builder` — validates required fields and constructs `DeploymentSpec`
//! 4. `revision` — parses S3/GitHub/Local revision details

mod builder;
mod envelope;
pub mod error;
pub mod parse;
mod revision;
pub mod types;

pub use error::{DeploymentSpecError, Result};
pub use types::{
    DEFAULT_APP_SPEC_PATH, DEFAULT_DEPLOYMENT_CREATOR, DEFAULT_DEPLOYMENT_TYPE,
    DEFAULT_FILE_EXISTS_BEHAVIOR, DeploymentSpec, Envelope, RevisionLocation, RevisionSource,
};
