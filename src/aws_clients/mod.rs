//! @risk none
//!
//! AWS client modules.
pub mod aws_client;
pub mod codedeploy_command_client;
pub mod credentials;
pub mod imds;
pub mod s3_client;
pub mod ssl;

pub use aws_client::AwsClient;
pub use codedeploy_command_client::CodeDeployCommandClient;
pub use credentials::{CredentialMode, Credentials};
pub use s3_client::{S3Client, S3ClientConfig};
