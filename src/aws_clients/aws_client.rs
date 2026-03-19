//! @risk none
//!
//! AWS client trait definition.
use crate::aws_clients::credentials::Credentials;

/// Generic AWS client trait for common AWS operations
pub trait AwsClient: Send + Sync {
    /// Get the AWS region
    fn region(&self) -> &str;

    /// Get the credentials
    fn credentials(&self) -> &Credentials;
}
