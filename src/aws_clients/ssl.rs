//! @risk medium
//!
//! TLS endpoint verification.
//!
//! Despite the name "SSL certificate field verification" in the Ruby agent,
//! this is really just a TLS connectivity pre-check. The Rust AWS SDK already
//! does `VERIFY_PEER` on every request, so the main value is failing fast at
//! startup rather than failing on the first poll.
//!
//! Ruby source: `CodeDeployControlCertVerifier#verify_cert` in
//! `lib/instance_agent/plugins/codedeploy/codedeploy_control.rb`
//! — makes an HTTPS GET to the endpoint with `VERIFY_PEER` mode.
//! If the TLS handshake fails, the agent aborts.

use std::time::Duration;

/// Verify TLS connectivity to the `CodeDeploy` endpoint.
///
/// Makes a single HTTPS request to confirm the TLS handshake succeeds
/// with a trusted CA. We don't inspect certificate fields — standard
/// TLS verification is sufficient.
///
/// # Errors
/// Returns an error message if the TLS connection cannot be established.
pub fn verify_tls_connection(endpoint: &str, proxy_uri: Option<&str>) -> Result<(), String> {
    let mut builder = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .danger_accept_invalid_certs(false); // explicit: enforce TLS verification

    if let Some(proxy) = proxy_uri {
        let p = reqwest::Proxy::all(proxy).map_err(|e| format!("invalid proxy URI: {e}"))?;
        builder = builder.proxy(p);
    }

    let client = builder.build().map_err(|e| format!("failed to build HTTP client: {e}"))?;

    // Ruby does `client.get '/'` — we do the same. The response status doesn't
    // matter; we only care that the TLS handshake completed successfully.
    match client.get(endpoint).send() {
        Err(e) if e.is_connect() || e.is_timeout() => {
            Err(format!("TLS connection to {endpoint} failed: {e}"))
        },
        // Non-TLS errors (e.g. HTTP status errors) mean TLS succeeded — that's fine.
        Ok(_) | Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_tls_connection_with_invalid_endpoint_fails() {
        let result = verify_tls_connection("https://localhost:1", None);
        assert!(result.is_err(), "expected TLS probe to fail for unreachable host");
    }

    #[test]
    fn verify_tls_connection_with_invalid_proxy_fails() {
        let result = verify_tls_connection("https://example.com", Some("not://valid"));
        assert!(result.is_err(), "expected error for invalid proxy URI");
    }
}
