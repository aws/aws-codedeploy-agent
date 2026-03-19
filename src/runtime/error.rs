//! @risk low
//!
//! Shared error type for runtime traits.
use std::fmt;

/// Error type for runtime trait operations (state store, downloader, AWS client).
#[derive(Debug)]
pub enum RuntimeError {
    /// An I/O operation failed.
    Io(std::io::Error),
    /// A generic runtime failure with a descriptive message.
    Other(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Other(_) => None,
        }
    }
}

impl From<std::io::Error> for RuntimeError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error;

    #[test]
    fn io_variant_displays_inner_message() {
        let err = RuntimeError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "gone"));
        assert!(err.to_string().contains("gone"));
    }

    #[test]
    fn other_variant_displays_message() {
        let err = RuntimeError::Other("something broke".into());
        assert_eq!(err.to_string(), "something broke");
    }

    #[test]
    fn io_variant_has_source() {
        let err = RuntimeError::Io(std::io::Error::new(std::io::ErrorKind::Other, "inner"));
        assert!(err.source().is_some());
    }

    #[test]
    fn other_variant_has_no_source() {
        let err = RuntimeError::Other("msg".into());
        assert!(err.source().is_none());
    }

    #[test]
    fn converts_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied");
        let err: RuntimeError = io_err.into();
        assert!(err.to_string().contains("denied"));
    }

    #[test]
    fn is_debuggable() {
        let err = RuntimeError::Other("test".into());
        let debug = format!("{err:?}");
        assert!(debug.contains("test"));
    }

    #[test]
    fn is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RuntimeError>();
    }
}
