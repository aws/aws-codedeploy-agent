//! @risk none
//!
//! Ping command — liveness check.

use serde_json::{Value, json};

/// Returns an empty success response.
#[must_use]
pub fn handle(_args: &Value) -> Value {
    json!({"ok": true})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_returns_ok() {
        let resp = handle(&json!({}));
        assert_eq!(resp["ok"], true);
    }
}
