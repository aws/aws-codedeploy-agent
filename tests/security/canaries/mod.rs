// Canary tests for security test objectives — run nightly/weekly with --include-ignored.
// These tests require infrastructure dependencies: TLS servers, IMDS simulation,
// or resource-intensive operations. All tests are marked #[ignore].

pub mod imds_proxy_tests;
pub mod resource_exhaustion;
pub mod tls_endpoint_tests;
