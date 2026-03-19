// Integration tests for security test objectives — run on every PR.
// Uses wiremock for HTTP mocking, tempfile for isolated directories,
// and real process spawning for lifecycle script tests.

mod appspec_validation;
mod archive_traversal;
mod installer_permissions;
mod state_config_logging_boundaries;
