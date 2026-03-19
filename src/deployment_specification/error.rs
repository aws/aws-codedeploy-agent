//! @risk none
//!
//! Deployment specification error types.
use std::fmt;

#[derive(Debug)]
pub enum DeploymentSpecError {
    MissingField(String),
    InvalidRevision(String),
    InvalidBundleType(String),
    InvalidFormat(String),
    SignatureVerification(String),
    ParseError(String),
}

impl fmt::Display for DeploymentSpecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingField(field) => write!(f, "Deployment Spec has no {field}"),
            Self::InvalidRevision(msg) | Self::InvalidBundleType(msg) => write!(f, "{msg}"),
            Self::InvalidFormat(msg) => {
                write!(f, "Unsupported DeploymentSpecification format: {msg}")
            },
            Self::SignatureVerification(msg) => write!(f, "Signature verification failed: {msg}"),
            Self::ParseError(msg) => write!(f, "Parse error: {msg}"),
        }
    }
}

impl std::error::Error for DeploymentSpecError {}

pub type Result<T> = std::result::Result<T, DeploymentSpecError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_field_display() {
        let err = DeploymentSpecError::MissingField("revision".to_string());
        assert_eq!(err.to_string(), "Deployment Spec has no revision");
    }

    #[test]
    fn invalid_revision_display() {
        let err = DeploymentSpecError::InvalidRevision("invalid format".to_string());
        assert_eq!(err.to_string(), "invalid format");
    }

    #[test]
    fn invalid_bundle_type_display() {
        let err = DeploymentSpecError::InvalidBundleType("unknown type".to_string());
        assert_eq!(err.to_string(), "unknown type");
    }

    #[test]
    fn invalid_format_display() {
        let err = DeploymentSpecError::InvalidFormat("JSON".to_string());
        assert_eq!(err.to_string(), "Unsupported DeploymentSpecification format: JSON");
    }

    #[test]
    fn signature_verification_display() {
        let err = DeploymentSpecError::SignatureVerification("invalid signature".to_string());
        assert_eq!(err.to_string(), "Signature verification failed: invalid signature");
    }

    #[test]
    fn parse_error_display() {
        let err = DeploymentSpecError::ParseError("malformed JSON".to_string());
        assert_eq!(err.to_string(), "Parse error: malformed JSON");
    }

    #[test]
    fn error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DeploymentSpecError>();
    }
}
