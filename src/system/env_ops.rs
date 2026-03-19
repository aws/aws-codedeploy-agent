//! @risk low
//!
//! Testable environment variable access.
/// Environment variable access abstraction.
///
/// Production code uses `SystemEnvOps` which reads real env vars.
/// Tests use `MockEnvOps` to control values without unsafe `set_var`.
pub trait EnvOps: Send + Sync {
    fn get(&self, key: &str) -> Option<String>;
}

/// Reads from the real process environment.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemEnvOps;

impl EnvOps for SystemEnvOps {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

#[cfg(test)]
pub use mock::MockEnvOps;

#[cfg(test)]
mod mock {
    use super::EnvOps;
    use std::collections::HashMap;

    /// Test-only env ops with preconfigured values.
    #[derive(Debug, Default, Clone)]
    pub struct MockEnvOps {
        vars: HashMap<String, String>,
    }

    impl MockEnvOps {
        pub fn with(key: &str, value: &str) -> Self {
            let mut vars = HashMap::new();
            vars.insert(key.to_string(), value.to_string());
            Self { vars }
        }
    }

    impl EnvOps for MockEnvOps {
        fn get(&self, key: &str) -> Option<String> {
            self.vars.get(key).cloned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_env_reads_path() {
        let ops = SystemEnvOps;
        // PATH is always set
        assert!(ops.get("PATH").is_some());
    }

    #[test]
    fn system_env_missing_key() {
        let ops = SystemEnvOps;
        assert!(ops.get("DEFINITELY_NOT_SET_12345").is_none());
    }

    #[test]
    fn mock_env_returns_configured_value() {
        let ops = MockEnvOps::with("MY_KEY", "my_value");
        assert_eq!(ops.get("MY_KEY"), Some("my_value".to_string()));
    }

    #[test]
    fn mock_env_missing_key() {
        let ops = MockEnvOps::default();
        assert!(ops.get("ANYTHING").is_none());
    }
}
