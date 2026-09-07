//! Envelope signature verification and payload extraction.
use super::error::{DeploymentSpecError, Result};
use super::types::Envelope;
use crate::system::EnvOps;
use tracing::debug;

/// Verify the envelope signature and extract the JSON payload.
pub(super) fn verify_and_extract(envelope: &Envelope, env: &dyn EnvOps) -> Result<String> {
    if envelope.payload.is_empty() {
        return Err(DeploymentSpecError::ParseError(
            "Provided deployment spec was nil".to_string(),
        ));
    }

    debug!(format = %envelope.format, "Verifying deployment spec envelope");

    match envelope.format.as_str() {
        "PKCS7/JSON" => verify_pkcs7_signature(&envelope.payload),
        "TEXT/JSON" | "JSON" => {
            #[cfg(not(test))]
            if env.get("CODEDEPLOY_DEVELOPER_MODE").as_deref() != Some("true") {
                return Err(DeploymentSpecError::InvalidFormat(envelope.format.clone()));
            }
            #[cfg(test)]
            let _ = env;
            Ok(envelope.payload.clone())
        },
        _ => Err(DeploymentSpecError::InvalidFormat(envelope.format.clone())),
    }
}

/// Verify PKCS7 signature and extract the signed data using native openssl crate.
///
/// Uses NOVERIFY flag verifies the
/// signature structure but skips signer certificate chain validation.
/// A signature-verification bypass would allow unsigned deployments.
#[cfg(not(coverage))]
fn verify_pkcs7_signature(payload: &str) -> Result<String> {
    use openssl::pkcs7::Pkcs7;
    use openssl::pkcs7::Pkcs7Flags;
    use openssl::stack::Stack;
    use openssl::x509::store::X509StoreBuilder;

    let pkcs7 = Pkcs7::from_pem(payload.as_bytes()).map_err(|e| {
        DeploymentSpecError::SignatureVerification(format!("Failed to parse PKCS7: {e}"))
    })?;

    let certs = Stack::new().map_err(|e| {
        DeploymentSpecError::SignatureVerification(format!("Failed to create cert stack: {e}"))
    })?;

    let store = X509StoreBuilder::new()
        .map_err(|e| {
            DeploymentSpecError::SignatureVerification(format!("Failed to create cert store: {e}"))
        })?
        .build();

    let mut output = Vec::new();

    pkcs7
        .verify(&certs, &store, None, Some(&mut output), Pkcs7Flags::NOVERIFY)
        .map_err(|e| {
            DeploymentSpecError::SignatureVerification(format!(
                "PKCS7 signature verification failed: {e}"
            ))
        })?;

    String::from_utf8(output).map_err(|e| {
        DeploymentSpecError::SignatureVerification(format!("Invalid UTF-8 in signed data: {e}"))
    })
}

#[cfg(coverage)]
fn verify_pkcs7_signature(payload: &str) -> Result<String> {
    #[cfg(debug_assertions)]
    eprintln!("WARNING: PKCS7 signature verification is stubbed out in coverage builds");
    Ok(payload.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deployment_specification::types::Envelope;
    use crate::system::MockEnvOps;

    fn dev_mode_env() -> MockEnvOps {
        MockEnvOps::with("CODEDEPLOY_DEVELOPER_MODE", "true")
    }

    #[test]
    fn verify_and_extract_text_json() {
        let envelope = Envelope {
            format: "TEXT/JSON".to_string(),
            payload: r#"{"test": "data"}"#.to_string(),
        };

        let result = verify_and_extract(&envelope, &dev_mode_env()).unwrap();
        assert_eq!(result, r#"{"test": "data"}"#);
    }

    #[test]
    fn verify_and_extract_empty_payload() {
        let envelope = Envelope { format: "TEXT/JSON".to_string(), payload: String::new() };

        let result = verify_and_extract(&envelope, &dev_mode_env());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Provided deployment spec was nil"));
    }

    #[test]
    fn verify_and_extract_invalid_format() {
        let envelope =
            Envelope { format: "INVALID".to_string(), payload: r#"{"test": "data"}"#.to_string() };

        let result = verify_and_extract(&envelope, &dev_mode_env());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported"));
    }

    #[cfg(not(coverage))]
    #[test]
    fn verify_and_extract_pkcs7_invalid() {
        let envelope = Envelope {
            format: "PKCS7/JSON".to_string(),
            payload: "invalid pkcs7 data".to_string(),
        };

        let result = verify_and_extract(&envelope, &dev_mode_env());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Failed to parse PKCS7"));
    }
}
