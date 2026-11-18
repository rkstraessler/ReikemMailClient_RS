use std::fmt;
use std::time::Duration;

use reqwest::Url;
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};

use crate::Error;

const MAX_TIMEOUT_SECONDS: u64 = 2_147_483;

/// Validated credentials, endpoint root and timeouts for an [`EmailApiClient`](crate::EmailApiClient).
pub struct ClientConfig {
    pub(crate) endpoint_root: Url,
    pub(crate) headers: HeaderMap,
    pub(crate) connect_timeout: Duration,
    pub(crate) request_timeout: Duration,
}

impl ClientConfig {
    /// Configures the HTTP(S) origin and optional reverse proxy path.
    ///
    /// Existing `/api/send`, `/api/v2/send` and `/api/v2` suffixes are accepted.
    /// The defaults are 10 seconds to connect and 30 seconds for the request.
    /// Use HTTPS for real credentials.
    ///
    /// # Errors
    /// Returns an error for invalid URLs or credentials outside printable,
    /// non-space ASCII.
    pub fn new(base_url: &str, auth_key: &str, adapter_key: &str) -> Result<Self, Error> {
        let mut endpoint_root = Url::parse(base_url).map_err(|_| Error::InvalidBaseUrl)?;
        if !matches!(endpoint_root.scheme(), "http" | "https")
            || !endpoint_root.username().is_empty()
            || endpoint_root.password().is_some()
            || endpoint_root.query().is_some_and(|query| !query.is_empty())
            || endpoint_root
                .fragment()
                .is_some_and(|fragment| !fragment.is_empty())
        {
            return Err(Error::InvalidBaseUrl);
        }

        let path = endpoint_root.path().trim_end_matches('/');
        let prefix = path
            .strip_suffix("/api/v2/send")
            .or_else(|| path.strip_suffix("/api/send"))
            .unwrap_or(path);
        let prefix = prefix.strip_suffix("/api/v2").unwrap_or(prefix);
        endpoint_root.set_path(&format!("{prefix}/api/v2/"));
        endpoint_root.set_query(None);
        endpoint_root.set_fragment(None);

        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            credential("Authorization", auth_key, "Bearer ")?,
        );
        headers.insert(
            "x-adapter-key",
            credential("X-Adapter-Key", adapter_key, "")?,
        );

        Ok(Self {
            endpoint_root,
            headers,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(30),
        })
    }

    /// Sets the connect timeout, including TLS, and the total HTTP request timeout.
    /// The total timeout includes reading the response body.
    ///
    /// # Errors
    /// Both values must be whole seconds in `1..=2_147_483`.
    pub fn with_timeouts(mut self, connect: Duration, request: Duration) -> Result<Self, Error> {
        if [connect, request].into_iter().any(|timeout| {
            timeout.is_zero()
                || timeout.subsec_nanos() != 0
                || timeout.as_secs() > MAX_TIMEOUT_SECONDS
        }) {
            return Err(Error::InvalidTimeout);
        }
        self.connect_timeout = connect;
        self.request_timeout = request;
        Ok(self)
    }
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClientConfig")
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .finish_non_exhaustive()
    }
}

fn credential(header: &'static str, key: &str, prefix: &str) -> Result<HeaderValue, Error> {
    if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(Error::InvalidCredential { header });
    }
    let mut value = HeaderValue::from_str(&format!("{prefix}{key}"))
        .map_err(|_| Error::InvalidCredential { header })?;
    value.set_sensitive(true);
    Ok(value)
}
