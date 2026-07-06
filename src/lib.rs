//! AWS `CodeDeploy` Agent - Rust Implementation
//!
//! This crate provides the Rust implementation of the AWS `CodeDeploy` Agent
//! for improved performance and type safety.

#![deny(clippy::all)]
#![warn(clippy::pedantic)]
#![allow(missing_docs)]
#![warn(missing_debug_implementations, unreachable_pub)]
#![cfg_attr(not(windows), forbid(unsafe_code))]
#![cfg_attr(windows, deny(unsafe_code))]

pub mod application_specification;
pub mod aws_clients;
pub mod command_poller;
pub mod command_port;
pub mod config;
pub mod daemon;
pub mod deployment_specification;
pub mod host_command;
pub mod installer;
pub mod lifecycle_event;
pub mod logging;
pub mod paths;
pub mod runtime;
pub mod string_utils;
pub mod system;
