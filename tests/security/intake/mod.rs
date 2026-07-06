// Integration tests for security test objectives — run on every PR.
// Uses wiremock for HTTP mocking, tempfile for isolated directories,
// and real process spawning for lifecycle script tests.

mod appspec_validation;
mod archive_dos_integrity;
mod archive_traversal;
mod command_port_dos;
mod concurrency;
mod deployment_flow;
mod imds_config_permissions;
mod installer_permissions;
mod process_isolation;
mod state_config_logging_boundaries;
mod token_credential_security;
