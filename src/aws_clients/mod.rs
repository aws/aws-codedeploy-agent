//! AWS client modules.
pub mod aws_client;
pub mod codedeploy_command_client;
pub mod credentials;
pub mod file_credentials;
pub mod imds;
pub mod s3_client;
pub mod ssl;
pub mod throttle_gate;
pub mod wire_log;

pub use aws_client::AwsClient;
pub use codedeploy_command_client::CodeDeployCommandClient;
pub use credentials::{CredentialMode, Credentials};
pub use file_credentials::FileCredentialProvider;
pub use s3_client::{S3Client, S3ClientConfig};
pub use throttle_gate::ThrottleGate;
