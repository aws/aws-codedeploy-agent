//! Error types for the `CodeDeploy` Command Service client.

use std::fmt;

/// Client error.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    message: String,
}

impl Error {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }

    /// The error kind.
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }

    /// Parse error from HTTP response body.
    pub(crate) fn from_response(status: u16, body: &[u8]) -> Self {
        // Detect throttling (HTTP 429) before parsing the body — the service
        // may return 429 with or without a JSON body.
        if status == 429 {
            let message = serde_json::from_slice::<serde_json::Value>(body)
                .ok()
                .and_then(|j| {
                    j.get("message")
                        .or_else(|| j.get("Message"))
                        .and_then(|v| v.as_str())
                        .map(String::from)
                })
                .unwrap_or_else(|| "Rate exceeded".to_string());
            return Self::new(ErrorKind::Throttled, message);
        }

        let Ok(json) = serde_json::from_slice::<serde_json::Value>(body) else {
            let body_str = String::from_utf8_lossy(body);
            let truncated = &body_str[..body_str.floor_char_boundary(512)];
            return Self::new(ErrorKind::Http, format!("HTTP {status}: {truncated}"));
        };

        // AWS `__type` can be namespace-prefixed with `#` (e.g. "com.amazonaws.codedeploy#ClientException")
        // or colon-suffixed (e.g. "ClientException:http://..."). Strip both to extract the bare type name.
        let Some(error_type) = json.get("__type").and_then(|v| v.as_str()) else {
            let body_str = json.to_string();
            let truncated = &body_str[..body_str.floor_char_boundary(512)];
            return Self::new(ErrorKind::Http, format!("HTTP {status}: {truncated}"));
        };
        let error_type = error_type.split('#').next_back().unwrap_or(error_type);
        let error_type = error_type.split(':').next().unwrap_or(error_type);

        // Kept as Option so throttle detection below keys off the real message,
        // not the synthesized fallback (which embeds the body).
        let raw_message =
            json.get("message").or_else(|| json.get("Message")).and_then(|v| v.as_str());

        // Detect throttling by __type or message, even on non-429 status codes.
        // The CodeDeploy service can return throttling with 400/503 + various __type values.
        let kind = match error_type {
            "ThrottlingException"
            | "TooManyRequestsException"
            | "Throttling"
            | "RequestThrottled"
            | "RequestThrottledException" => ErrorKind::Throttled,
            _ if raw_message
                .is_some_and(|m| m.contains("Rate exceeded") || m.contains("rate exceeded")) =>
            {
                ErrorKind::Throttled
            },
            "ClientException" => ErrorKind::ClientException,
            "ServerException" => ErrorKind::ServerException,
            _ => ErrorKind::Http,
        };

        // No message field: preserve the __type and raw body instead of a
        // bare "HTTP {status}".
        let message = if let Some(m) = raw_message {
            m.to_string()
        } else {
            let body_str = json.to_string();
            let truncated = &body_str[..body_str.floor_char_boundary(512)];
            format!("HTTP {status} [type={error_type}]: {truncated}")
        };

        Self::new(kind, message)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl std::error::Error for Error {}

impl From<reqwest::Error> for Error {
    fn from(err: reqwest::Error) -> Self {
        Self::new(ErrorKind::Network, err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Self::new(ErrorKind::Deserialization, err.to_string())
    }
}

/// Error classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    /// Client-side request error (invalid input).
    ClientException,
    /// Server-side failure.
    ServerException,
    /// HTTP or network error.
    Http,
    /// Request signing failed.
    Signing,
    /// Response deserialization failed.
    Deserialization,
    /// Request serialization failed.
    Serialization,
    /// Network communication error.
    Network,
    /// Client construction failed.
    Build,
    /// Rate limit exceeded (HTTP 429).
    Throttled,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClientException => write!(f, "ClientException"),
            Self::ServerException => write!(f, "ServerException"),
            Self::Http => write!(f, "HttpError"),
            Self::Signing => write!(f, "SigningError"),
            Self::Deserialization => write!(f, "DeserializationError"),
            Self::Serialization => write!(f, "SerializationError"),
            Self::Network => write!(f, "NetworkError"),
            Self::Build => write!(f, "BuildError"),
            Self::Throttled => write!(f, "Throttled"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_response_with_client_exception() {
        let body = r#"{"__type": "ClientException", "message": "Invalid request"}"#;
        let error = Error::from_response(400, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::ClientException);
        assert_eq!(error.message, "Invalid request");
    }

    #[test]
    fn from_response_with_server_exception() {
        let body = r#"{"__type": "ServerException", "message": "Internal error"}"#;
        let error = Error::from_response(500, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::ServerException);
        assert_eq!(error.message, "Internal error");
    }

    #[test]
    fn from_response_with_prefixed_type() {
        let body =
            r#"{"__type": "com.amazonaws.codedeploy#ClientException", "message": "Bad input"}"#;
        let error = Error::from_response(400, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::ClientException);
        assert_eq!(error.message, "Bad input");
    }

    #[test]
    fn from_response_missing_type_falls_back_to_http() {
        let body = r#"{"message": "Some error"}"#;
        let error = Error::from_response(404, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::Http);
        assert!(error.message.starts_with("HTTP 404: "), "got: {}", error.message);
        assert!(error.message.contains("Some error"), "got: {}", error.message);
    }

    #[test]
    fn from_response_invalid_json_falls_back_to_http() {
        let body = b"not json";
        let error = Error::from_response(500, body);
        assert_eq!(error.kind(), &ErrorKind::Http);
        assert_eq!(error.message, "HTTP 500: not json");
    }

    #[test]
    fn from_response_extracts_capital_message_field() {
        let body = r#"{"__type": "ServerException", "Message": "Capital M message"}"#;
        let error = Error::from_response(500, body.as_bytes());
        assert_eq!(error.message, "Capital M message");
    }

    #[test]
    fn from_response_no_message_preserves_type_and_body() {
        let body = r#"{"__type": "ClientException"}"#;
        let error = Error::from_response(400, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::ClientException);
        // The bare "HTTP 400" message used to discard the __type that was
        // already parsed; now it is preserved alongside the raw body.
        assert!(
            error.message.starts_with("HTTP 400 [type=ClientException]: "),
            "got: {}",
            error.message
        );
        assert!(error.message.contains("__type"), "got: {}", error.message);
    }

    #[test]
    fn from_response_namespaced_type_no_message_preserves_bare_type() {
        // __type is namespace-prefixed; the preserved diagnostic uses the bare name.
        let body = r#"{"__type": "com.amazonaws.codedeploy#ClientException"}"#;
        let error = Error::from_response(400, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::ClientException);
        assert!(
            error.message.starts_with("HTTP 400 [type=ClientException]: "),
            "got: {}",
            error.message
        );
    }

    #[test]
    fn from_response_with_colon_suffixed_type() {
        let body =
            r#"{"__type": "ClientException:http://internal.amazonaws.com/doc", "message": "test"}"#;
        let error = Error::from_response(400, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::ClientException);
        assert_eq!(error.message, "test");
    }

    #[test]
    fn from_serde_json_error() {
        let json_err = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let error = Error::from(json_err);
        assert_eq!(error.kind(), &ErrorKind::Deserialization);
    }

    #[test]
    fn display_formatting_works() {
        let error = Error::new(ErrorKind::ClientException, "test message");
        assert_eq!(error.to_string(), "ClientException: test message");
    }

    #[test]
    fn error_kind_display_formatting() {
        assert_eq!(ErrorKind::ClientException.to_string(), "ClientException");
        assert_eq!(ErrorKind::ServerException.to_string(), "ServerException");
        assert_eq!(ErrorKind::Http.to_string(), "HttpError");
        assert_eq!(ErrorKind::Signing.to_string(), "SigningError");
        assert_eq!(ErrorKind::Deserialization.to_string(), "DeserializationError");
        assert_eq!(ErrorKind::Serialization.to_string(), "SerializationError");
        assert_eq!(ErrorKind::Network.to_string(), "NetworkError");
        assert_eq!(ErrorKind::Build.to_string(), "BuildError");
        assert_eq!(ErrorKind::Throttled.to_string(), "Throttled");
    }

    #[test]
    fn from_response_429_is_throttled() {
        let body = r#"{"__type": "ClientException", "message": "Rate exceeded"}"#;
        let error = Error::from_response(429, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::Throttled);
        assert_eq!(error.message, "Rate exceeded");
    }

    #[test]
    fn from_response_429_no_body_is_throttled() {
        let error = Error::from_response(429, b"not json");
        assert_eq!(error.kind(), &ErrorKind::Throttled);
        assert_eq!(error.message, "Rate exceeded");
    }

    #[test]
    fn error_new_constructor() {
        let error = Error::new(ErrorKind::Http, "test");
        assert_eq!(error.kind(), &ErrorKind::Http);
        assert_eq!(error.message, "test");
    }

    #[test]
    fn from_reqwest_error() {
        // Create a reqwest error by making an invalid request
        let err = reqwest::blocking::Client::new()
            .get("http://invalid-domain-that-does-not-exist-12345.com")
            .send()
            .unwrap_err();
        let error = Error::from(err);
        assert_eq!(error.kind(), &ErrorKind::Network);
    }

    #[test]
    fn error_kind_accessor() {
        let error = Error::new(ErrorKind::Signing, "test");
        assert_eq!(error.kind(), &ErrorKind::Signing);
    }

    #[test]
    fn from_response_throttling_exception_type_is_throttled() {
        let body = r#"{"__type": "ThrottlingException", "message": "Rate exceeded"}"#;
        let error = Error::from_response(400, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::Throttled);
        assert_eq!(error.message, "Rate exceeded");
    }

    #[test]
    fn from_response_too_many_requests_type_is_throttled() {
        let body = r#"{"__type": "TooManyRequestsException", "message": "Too many requests"}"#;
        let error = Error::from_response(503, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::Throttled);
    }

    #[test]
    fn from_response_rate_exceeded_message_is_throttled() {
        // Unknown __type but message says "Rate exceeded"
        let body = r#"{"__type": "UnknownType", "message": "Rate exceeded"}"#;
        let error = Error::from_response(400, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::Throttled);
        assert_eq!(error.message, "Rate exceeded");
    }

    #[test]
    fn from_response_rate_exceeded_case_insensitive_message() {
        let body = r#"{"__type": "SomeException", "message": "Request rate exceeded limit"}"#;
        let error = Error::from_response(400, body.as_bytes());
        assert_eq!(error.kind(), &ErrorKind::Throttled);
    }
}
