use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use time::{Date, Month, OffsetDateTime};

use crate::Error;
use crate::validation::js_whitespace;

/// An identifier accepted as a signed integer or decimal string.
/// The API receives its canonical positive signed 64-bit decimal representation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiId {
    /// An exactly represented Rust integer.
    Number(i64),
    /// A decimal string, optionally with surrounding whitespace and leading zeroes.
    Text(String),
}

impl From<i64> for ApiId {
    fn from(value: i64) -> Self {
        Self::Number(value)
    }
}

impl From<&str> for ApiId {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for ApiId {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl ApiId {
    pub(crate) fn positive_value(&self, field: &'static str) -> Result<i64, Error> {
        match self {
            Self::Number(value) if *value > 0 => Ok(*value),
            Self::Number(_) => Err(Error::input(
                field,
                "expected a positive signed 64-bit integer",
            )),
            Self::Text(value) => parse_positive(value, field),
        }
    }
}

/// A history date as an exact calendar date or an instant interpreted in UTC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryDate {
    /// A `YYYY-MM-DD` date in years 0001 through 9999.
    Text(String),
    /// An instant whose UTC calendar date is used.
    Time(SystemTime),
}

impl From<&str> for HistoryDate {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<String> for HistoryDate {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<SystemTime> for HistoryDate {
    fn from(value: SystemTime) -> Self {
        Self::Time(value)
    }
}

/// One or several message IDs for a history query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryMessageIds {
    /// One numeric identifier.
    Single(i64),
    /// One or more comma-separated decimal strings.
    Csv(String),
    /// A list mixing numeric identifiers and decimal strings.
    List(Vec<ApiId>),
}

/// Date and ID filters for retrieving mail history.
#[derive(Debug, Clone, Default)]
pub struct GetMailHistoryOptions {
    /// Inclusive start date, required when no IDs are supplied.
    pub date_from: Option<HistoryDate>,
    /// Inclusive end date, when supplied.
    pub date_to: Option<HistoryDate>,
    /// IDs to look up, in input order after deduplication.
    pub message_ids: Option<HistoryMessageIds>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    date_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    date_to: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    message_ids: Vec<String>,
}

impl TryFrom<&GetMailHistoryOptions> for HistoryRequest {
    type Error = Error;

    fn try_from(options: &GetMailHistoryOptions) -> Result<Self, Error> {
        let message_ids = normalize_ids(options.message_ids.as_ref())?;
        let from = optional_date(options.date_from.as_ref(), "dateFrom")?;
        let to = optional_date(options.date_to.as_ref(), "dateTo")?;
        if from.is_none() && message_ids.is_empty() {
            return Err(Error::input(
                "dateFrom",
                "required when no message IDs are supplied",
            ));
        }
        if from.zip(to).is_some_and(|(from, to)| from > to) {
            return Err(Error::input("dateFrom", "must be on or before dateTo"));
        }
        Ok(Self {
            date_from: from.map(|date| date.to_string()),
            date_to: to.map(|date| date.to_string()),
            message_ids,
        })
    }
}

pub(crate) fn parse_positive(text: &str, field: &'static str) -> Result<i64, Error> {
    let digits = text.trim_matches(js_whitespace);
    if digits.bytes().all(|byte| byte.is_ascii_digit())
        && let Ok(value) = digits.parse::<i64>()
        && value > 0
    {
        return Ok(value);
    }
    Err(Error::input(
        field,
        "expected a positive signed 64-bit decimal integer",
    ))
}

fn normalize_ids(ids: Option<&HistoryMessageIds>) -> Result<Vec<String>, Error> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    let mut push = |value: i64| {
        if seen.insert(value) {
            normalized.push(value.to_string());
        }
    };

    match ids {
        None => {}
        Some(HistoryMessageIds::Single(value)) => {
            push(ApiId::Number(*value).positive_value("messageId")?)
        }
        Some(HistoryMessageIds::Csv(text)) => {
            for value in text.split(',').map(|part| part.trim_matches(js_whitespace)) {
                if !value.is_empty() {
                    push(parse_positive(value, "messageId")?);
                }
            }
        }
        Some(HistoryMessageIds::List(values)) => {
            for value in values {
                if !matches!(value, ApiId::Text(text) if text.is_empty()) {
                    push(value.positive_value("messageId")?);
                }
            }
        }
    }
    Ok(normalized)
}

fn optional_date(value: Option<&HistoryDate>, field: &'static str) -> Result<Option<Date>, Error> {
    let date = match value {
        None => return Ok(None),
        Some(HistoryDate::Text(value)) if value.is_empty() => return Ok(None),
        Some(HistoryDate::Text(value)) => parse_date(value.trim_matches(js_whitespace)),
        Some(HistoryDate::Time(value)) => {
            let nanos = match value.duration_since(UNIX_EPOCH) {
                Ok(duration) => duration.as_nanos() as i128,
                Err(error) => -(error.duration().as_nanos() as i128),
            };
            OffsetDateTime::from_unix_timestamp_nanos(nanos)
                .ok()
                .map(|instant| instant.date())
        }
    };
    date.filter(|date| (1..=9999).contains(&date.year()))
        .map(Some)
        .ok_or_else(|| Error::input(field, "expected a valid calendar date (YYYY-MM-DD)"))
}

fn parse_date(value: &str) -> Option<Date> {
    let (year, rest) = value.split_once('-')?;
    let (month, day) = rest.split_once('-')?;
    if year.len() != 4
        || month.len() != 2
        || day.len() != 2
        || ![year, month, day]
            .into_iter()
            .flat_map(str::bytes)
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    Date::from_calendar_date(
        year.parse().ok()?,
        Month::try_from(month.parse::<u8>().ok()?).ok()?,
        day.parse().ok()?,
    )
    .ok()
}
