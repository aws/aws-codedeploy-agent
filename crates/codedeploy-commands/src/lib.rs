//! AWS `CodeDeploy` Command Service client.
//!
//! Implements the private `CodeDeployCommandService_v20141006` JSON-RPC API
//! used by the `CodeDeploy` agent to poll for and report on deployments.

#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![forbid(unsafe_code)]

pub mod client;
pub mod endpoint;
pub mod error;
pub mod types;

pub use client::Client;
pub use error::{Error, ErrorKind};
pub use types::{
    CommandStatus, DeploymentSpecification, Envelope, GetDeploymentSpecificationOutput,
    HostCommandInstance,
};
