// Security test suite — validates 26 security test objectives (78 test cases)
// covering credential protection, input validation, process isolation,
// denial-of-service resistance, data integrity, and network security.
//
// Structure:
//   intake/   — integration tests run on every PR
//   canaries/ — infrastructure-dependent tests run nightly/weekly with --include-ignored

pub mod canaries;
pub mod fixtures;
pub mod helpers;
pub mod intake;
