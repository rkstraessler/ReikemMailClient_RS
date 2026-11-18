use std::io::ErrorKind;

/// A local failure before, during, or after a Mail API request.
/// Valid API envelopes, including non-2xx responses, are returned as values.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The base URL is invalid or includes unsupported URL components.
    #[error("invalid Mail API URL")]
    InvalidBaseUrl,
    /// A required authentication header value is invalid.
    #[error("invalid credential for {header}")]
    InvalidCredential {
        /// The header that could not be set.
        header: &'static str,
    },
    /// A timeout lies outside the supported range.
    #[error("timeouts must be positive whole seconds (at most 2147483)")]
    InvalidTimeout,
    /// A request field violates the API contract.
    #[error("invalid {field}: {reason}")]
    InvalidInput {
        /// The wire field associated with the failure.
        field: &'static str,
        /// A diagnostic that excludes the supplied value.
        reason: &'static str,
    },
    /// The Rustls HTTP client could not be initialized.
    #[error("could not initialize the Mail API HTTP client")]
    ClientInitialization,
    /// A validated request could not be encoded.
    #[error("could not encode the Mail API request")]
    RequestEncoding,
    /// The HTTP operation did not finish successfully.
    #[error("Mail API transport failed ({0})")]
    Transport(TransportErrorKind),
    /// The response was not valid UTF-8 JSON.
    #[error("Mail API returned invalid JSON (HTTP {http_code})")]
    InvalidJson {
        /// The actual HTTP status.
        http_code: u16,
    },
    /// JSON was received without the required status and data envelope.
    #[error("Mail API returned an invalid envelope (HTTP {http_code})")]
    InvalidEnvelope {
        /// The actual HTTP status.
        http_code: u16,
    },
    /// A successful download response contained JSON instead of a file.
    #[error("Mail API returned JSON instead of a file (HTTP {http_code})")]
    JsonInsteadOfFile {
        /// The actual HTTP status.
        http_code: u16,
    },
    /// A local attachment was not a readable regular file.
    #[error("attachment must be a readable file ({0})")]
    AttachmentRead(ErrorKind),
    /// The downloaded file could not be saved completely.
    #[error("cannot save the downloaded file completely ({0})")]
    DownloadWrite(ErrorKind),
}

/// The category of a failure in the HTTP transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TransportErrorKind {
    /// A connect or total request timeout elapsed.
    #[error("timeout")]
    Timeout,
    /// DNS, TCP, or TLS connection setup failed.
    #[error("connection")]
    Connection,
    /// The request or response body could not be transferred completely.
    #[error("body transfer")]
    Body,
    /// Another failure occurred while sending or receiving the request.
    #[error("request")]
    Request,
}

impl Error {
    pub(crate) fn input(field: &'static str, reason: &'static str) -> Self {
        Self::InvalidInput { field, reason }
    }

    pub(crate) fn transport(error: reqwest::Error) -> Self {
        let kind = if error.is_timeout() {
            TransportErrorKind::Timeout
        } else if error.is_connect() {
            TransportErrorKind::Connection
        } else if error.is_body() || error.is_decode() {
            TransportErrorKind::Body
        } else {
            TransportErrorKind::Request
        };
        Self::Transport(kind)
    }
}
