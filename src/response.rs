use std::collections::BTreeMap;

use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Map, Number, Value, value::RawValue};

use crate::{Error, MailAddress, Nullable};

const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
const MAX_JSON_DEPTH: usize = 128;

/// A number or string as returned by the API.
/// Large JSON numbers are converted to their original text by the client.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ApiInteger {
    /// A number in the safe JavaScript integer range.
    Number(Number),
    /// An API string or the exact token of a number outside the safe range.
    String(String),
}

impl ApiInteger {
    /// Returns the value as `i64` when it is an integer that fits.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Number(value) => value.as_i64(),
            Self::String(value) => value.parse().ok(),
        }
    }
}

/// A documented mail queue status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MailStatus {
    /// Filtering is in progress.
    Filtering,
    /// The message is queued.
    Queued,
    /// The message is held for review.
    Review,
    /// The message was rejected.
    Rejected,
    /// The message was sent.
    Sent,
    /// Delivery failed.
    Failed,
    /// The message is being prepared.
    Preparing,
    /// The message is being sent.
    Sending,
}

impl MailStatus {
    /// Returns all documented queue statuses.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[
            Self::Filtering,
            Self::Queued,
            Self::Review,
            Self::Rejected,
            Self::Sent,
            Self::Failed,
            Self::Preparing,
            Self::Sending,
        ]
    }

    /// Checks whether a string is a documented queue status.
    #[must_use]
    pub fn is_known(status: &str) -> bool {
        Self::all().iter().any(|value| value.as_str() == status)
    }

    /// Returns the status as spelled on the wire.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Filtering => "filtering",
            Self::Queued => "queued",
            Self::Review => "review",
            Self::Rejected => "rejected",
            Self::Sent => "sent",
            Self::Failed => "failed",
            Self::Preparing => "preparing",
            Self::Sending => "sending",
        }
    }
}

/// A queue status that preserves values introduced by newer servers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum QueueStatus {
    /// A status recognized by this SDK.
    Known(MailStatus),
    /// An unfamiliar status retained verbatim.
    Other(String),
}

impl QueueStatus {
    /// Returns the original wire value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        match self {
            Self::Known(status) => status.as_str(),
            Self::Other(status) => status,
        }
    }
}

/// A response field represented as an object or as an array by the server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ObjectOrArray<T> {
    /// The server's array representation, usually empty.
    Array(Vec<Value>),
    /// A typed object.
    Object(T),
}

/// One filter rejection.
#[derive(Debug, Clone, Deserialize)]
pub struct Rejection {
    /// Resulting queue status.
    pub status: Option<QueueStatus>,
    /// Machine-readable reason code.
    pub code: Option<String>,
    /// Explanation from the server.
    pub reason: Option<String>,
    /// Filter that produced the rejection.
    pub filter: Option<String>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Aggregate filtering information.
#[derive(Debug, Clone, Deserialize)]
pub struct FilterResults {
    /// Number of filtered entries.
    pub filtered: Option<ApiInteger>,
    /// Explanations returned by the filters.
    pub reasons: Option<Vec<String>>,
    /// Rejections indexed by the server's keys, or an empty array.
    pub rejections: Option<ObjectOrArray<BTreeMap<String, Rejection>>>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Retry metadata from the API's AI filter.
#[derive(Debug, Clone, Deserialize)]
pub struct AiRetry {
    /// Whether a retry is pending.
    pub pending: Option<bool>,
    /// Retry attempt number.
    pub attempt: Option<ApiInteger>,
    /// Time of the next attempt, in the server's format.
    #[serde(default)]
    pub next_attempt_at: Nullable<String>,
    /// Scheduled delay in seconds.
    pub delay_seconds: Option<ApiInteger>,
    /// HTTP status reported by the upstream AI service.
    #[serde(default)]
    pub http_status: Nullable<ApiInteger>,
    /// Retry-after duration in seconds.
    #[serde(default)]
    pub retry_after_seconds: Nullable<ApiInteger>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Decision metadata from the API's AI filter.
#[derive(Debug, Clone, Deserialize)]
pub struct AiResult {
    /// Filter decision.
    pub decision: Option<String>,
    /// Machine-readable reason code.
    pub reason_code: Option<String>,
    /// Confidence score from the server.
    pub confidence: Option<f64>,
    /// Explanation from the server.
    pub reason: Option<String>,
    /// Model identifier.
    pub model: Option<String>,
    /// Human-readable model name.
    pub model_display_name: Option<String>,
    /// Version of the prompt.
    pub prompt_version: Option<String>,
    /// Hash of the prompt.
    pub prompt_hash: Option<String>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Extended information about the API's AI filter.
#[derive(Debug, Clone, Deserialize)]
pub struct AiFilterDetails {
    /// Filter attempt number.
    pub attempt: Option<ApiInteger>,
    /// Timestamp of the last error, in the server's format.
    pub last_error_at: Option<String>,
    /// Server-side retry state.
    pub retry: Option<AiRetry>,
    /// Model decision metadata.
    pub result: Option<AiResult>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Extended filtering information returned with a mail entry.
#[derive(Debug, Clone, Deserialize)]
pub struct FilterDetails {
    /// Details from the AI filter.
    pub ai: Option<AiFilterDetails>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One queue status snapshot or history entry.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailEntry {
    /// Queue identifier.
    pub queue_id: Option<ApiInteger>,
    /// Message identifier.
    pub message_id: Option<String>,
    /// SMTP message identifier.
    pub smtp_message_id: Option<String>,
    /// Observed queue status.
    pub status: Option<QueueStatus>,
    /// Position within a result.
    pub index: Option<ApiInteger>,
    /// Whether the entry was successful.
    pub success: Option<bool>,
    /// Error information returned by the server.
    #[serde(default)]
    pub error: Nullable<Value>,
    /// Additional errors returned by the server.
    #[serde(default)]
    pub errors: Nullable<Value>,
    /// Legacy recipient value.
    pub recipient: Option<String>,
    /// Message subject.
    pub subject: Option<String>,
    /// HTML message body.
    pub html: Option<String>,
    /// Plain-text message body.
    #[serde(default)]
    pub text: Nullable<String>,
    /// Primary recipients.
    pub to: Option<Vec<MailAddress>>,
    /// CC recipients.
    pub cc: Option<Vec<MailAddress>>,
    /// BCC recipients.
    pub bcc: Option<Vec<MailAddress>>,
    /// Reply-to addresses.
    pub reply_to: Option<Vec<MailAddress>>,
    /// Business key associated with the message.
    #[serde(default)]
    pub idempotency_key: Nullable<String>,
    /// Legacy subject value.
    pub regard: Option<String>,
    /// Legacy message body.
    pub content: Option<String>,
    /// Aggregate filtering information.
    pub filters: Option<FilterResults>,
    /// Machine-readable code for a rejection.
    #[serde(default)]
    pub rejection_code: Nullable<String>,
    /// Explanation for a rejection.
    #[serde(default)]
    pub rejection_reason: Nullable<String>,
    /// Filter decision time, in the server's format.
    #[serde(default)]
    pub filtered_at: Nullable<String>,
    /// Scheduled filter retry, in the server's format.
    #[serde(default)]
    pub filter_next_attempt_at: Nullable<String>,
    /// Extended filtering information, which may be an empty array.
    pub filter_details: Option<ObjectOrArray<FilterDetails>>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Data returned by a send operation.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendData {
    /// Whether the operation succeeded according to the API.
    pub success: Option<bool>,
    /// Error information returned by the server.
    #[serde(default)]
    pub error: Nullable<Value>,
    /// Machine-readable error code such as `idempotency_conflict`.
    pub code: Option<String>,
    /// Queue identifiers.
    pub queue_ids: Option<Vec<ApiInteger>>,
    /// Message identifiers.
    pub message_ids: Option<Vec<String>>,
    /// SMTP message identifiers.
    pub smtp_message_ids: Option<Vec<String>>,
    /// Queue status snapshots.
    pub statuses: Option<Vec<MailEntry>>,
    /// Aggregate filtering information.
    pub filters: Option<FilterResults>,
    /// Business key received by the server.
    #[serde(default)]
    pub idempotency_key: Nullable<String>,
    /// Whether the API returned a saved idempotency snapshot.
    pub replayed: Option<bool>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One result within a personalized batch.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchResult {
    /// Zero-based position in the submitted batch.
    pub index: usize,
    /// Whether this message was accepted.
    pub success: bool,
    /// API result status for this message.
    pub status: i64,
    /// Data for this message.
    pub data: SendData,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Data returned by a personalized batch.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchData {
    /// Per-message results, including partially successful batches.
    pub results: Option<Vec<BatchResult>>,
    /// Aggregate send data and additional fields.
    #[serde(flatten)]
    pub send: SendData,
}

/// Mail history grouped by message direction.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MailHistory {
    /// Entries for mail sent through the API.
    pub outbound: Option<Vec<MailEntry>>,
    /// Entries for mail received by the API.
    pub inbound: Option<Vec<MailEntry>>,
    /// Additional API fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A general `results` value returned by the API.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum HistoryResults {
    /// Individual batch results.
    Batch(Vec<BatchResult>),
    /// Aggregate filter results.
    Filters(FilterResults),
}

/// Shared API data, including mail history and errors from the download operation.
#[derive(Debug, Clone, Deserialize)]
pub struct ApiData {
    /// Optional results associated with the operation.
    pub results: Option<HistoryResults>,
    /// History entries by direction.
    pub history: Option<MailHistory>,
    /// Shared send fields and additional data.
    #[serde(flatten)]
    pub send: SendData,
}

/// The contents of the API envelope's `data` field.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ApiPayload<T> {
    /// A response matching the operation's typed model.
    Object(T),
    /// The server returned array data, such as `[]` on an error.
    Array(Vec<Value>),
    /// Valid object data that does not match the current model.
    Unrecognized(Map<String, Value>),
}

impl<T> ApiPayload<T> {
    /// Returns typed data if the response matches the operation's model.
    #[must_use]
    pub fn as_object(&self) -> Option<&T> {
        match self {
            Self::Object(data) => Some(data),
            _ => None,
        }
    }
}

/// A valid API envelope with its original HTTP status and additional fields.
#[derive(Debug, Clone)]
pub struct ApiResponse<T = ApiData> {
    /// Status supplied in the JSON envelope.
    pub status: i64,
    /// Operation-specific response data.
    pub data: ApiPayload<T>,
    /// Actual HTTP status supplied by the server.
    pub http_code: u16,
    /// Optional error details from the envelope. Explicit JSON null is retained.
    pub error: Option<Value>,
    /// Optional machine-readable envelope code.
    pub code: Option<String>,
    /// Additional fields from the envelope.
    pub extra: Map<String, Value>,
}

impl<T> ApiResponse<T> {
    /// Checks whether the HTTP status is in the `2xx` range.
    #[must_use]
    pub fn is_http_success(&self) -> bool {
        (200..300).contains(&self.http_code)
    }
}

impl ApiResponse<SendData> {
    /// Checks whether the API returned HTTP 409 due to reuse of an idempotency key.
    #[must_use]
    pub fn is_idempotency_conflict(&self) -> bool {
        if self.http_code != 409 {
            return false;
        }
        let code = match &self.data {
            ApiPayload::Object(data) => data.code.as_deref(),
            ApiPayload::Unrecognized(data) => data.get("code").and_then(Value::as_str),
            ApiPayload::Array(_) => None,
        };
        code.or(self.code.as_deref()) == Some("idempotency_conflict")
    }
}

/// Response from a single or grouped send.
pub type SendResponse = ApiResponse<SendData>;

/// Response from a personalized batch send.
pub type BatchResponse = ApiResponse<BatchData>;

pub(crate) fn decode<T: DeserializeOwned>(
    body: &[u8],
    http_code: u16,
) -> Result<ApiResponse<T>, Error> {
    let json = parse_json(body).ok_or(Error::InvalidJson { http_code })?;
    let Value::Object(mut object) = json else {
        return Err(Error::InvalidEnvelope { http_code });
    };
    let status = object
        .remove("status")
        .and_then(|status| status.as_i64())
        .ok_or(Error::InvalidEnvelope { http_code })?;
    let data = match object.remove("data") {
        Some(value @ Value::Object(_)) => match T::deserialize(&value) {
            Ok(data) => ApiPayload::Object(data),
            Err(_) => match value {
                Value::Object(object) => ApiPayload::Unrecognized(object),
                _ => return Err(Error::InvalidEnvelope { http_code }),
            },
        },
        Some(Value::Array(array)) => ApiPayload::Array(array),
        _ => return Err(Error::InvalidEnvelope { http_code }),
    };
    object.remove("httpCode");
    let code = if object.get("code").is_some_and(Value::is_string) {
        object
            .remove("code")
            .and_then(|code| code.as_str().map(str::to_owned))
    } else {
        None
    };
    Ok(ApiResponse {
        status,
        data,
        http_code,
        error: object.remove("error"),
        code,
        extra: object,
    })
}

pub(crate) fn looks_like_envelope(body: &[u8]) -> bool {
    parse_json(body).is_some_and(|json| {
        json.get("status").and_then(Value::as_i64).is_some()
            && json
                .get("data")
                .is_some_and(|data| data.is_array() || data.is_object())
    })
}

fn parse_json(body: &[u8]) -> Option<Value> {
    let body = body.strip_prefix(b"\xef\xbb\xbf").unwrap_or(body);
    let raw = serde_json::from_slice::<&RawValue>(body).ok()?;
    decode_value(raw, 0)
}

fn decode_value(raw: &RawValue, depth: usize) -> Option<Value> {
    if depth >= MAX_JSON_DEPTH {
        return None;
    }
    let text = raw.get();
    match text.as_bytes().first()? {
        b'{' => {
            // RawValue retains number lexemes including exponent spelling and signs.
            let object = serde_json::from_str::<BTreeMap<String, &RawValue>>(text).ok()?;
            object
                .into_iter()
                .map(|(name, value)| Some((name, decode_value(value, depth + 1)?)))
                .collect::<Option<Map<_, _>>>()
                .map(Value::Object)
        }
        b'[' => {
            let values = serde_json::from_str::<Vec<&RawValue>>(text).ok()?;
            values
                .into_iter()
                .map(|value| decode_value(value, depth + 1))
                .collect::<Option<Vec<_>>>()
                .map(Value::Array)
        }
        b'-' | b'0'..=b'9' => {
            let number = text.parse::<f64>().ok()?;
            if !number.is_finite() || number.abs() > MAX_SAFE_INTEGER {
                Some(Value::String(text.to_owned()))
            } else if number.fract() == 0.0 {
                Some(Value::from(number as i64))
            } else {
                Number::from_f64(number).map(Value::Number)
            }
        }
        _ => serde_json::from_str(text).ok(),
    }
}
