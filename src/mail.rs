use std::collections::HashSet;
use std::net::{IpAddr, Ipv6Addr};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::attachments::{Upload, load_uploads};
use crate::history::parse_positive;
use crate::validation::{js_whitespace, php_trim};
use crate::{Attachment, EmailApiClient, Error, Nullable};

const MAX_DELAY_UNTIL: i64 = 253_402_300_799;

/// A recipient or reply-to mailbox with an optional display name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailAddress {
    /// ASCII mailbox address. Use Punycode for international domain names.
    pub email: String,
    /// Display name. An explicitly empty name is sent as `null`.
    #[serde(default, skip_serializing_if = "Nullable::is_missing")]
    pub name: Nullable<String>,
}

impl MailAddress {
    /// Creates an address with its display name omitted.
    pub fn new(email: impl Into<String>) -> Self {
        Self {
            email: email.into(),
            name: Nullable::Missing,
        }
    }
}

/// The earliest delivery time, represented as positive Epoch seconds or an instant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelayUntil {
    /// Whole seconds since the Unix epoch, at most `253_402_300_799`.
    Seconds(i64),
    /// Decimal representation of the same whole seconds.
    Text(String),
    /// A UTC instant whose fractional seconds are discarded.
    Time(SystemTime),
}

/// Options for one queued mail, possibly with multiple recipients.
#[derive(Debug, Clone)]
pub struct SendMailOptions {
    /// Required recipient list, with a maximum of 100 entries before deduplication.
    pub to: Vec<MailAddress>,
    /// Message subject, up to 5,000 Unicode characters without control characters.
    pub subject: String,
    /// Nonblank HTML body. Original whitespace and line endings are preserved.
    pub html: String,
    /// Optional business key for idempotent submission on the server.
    pub idempotency_key: Option<String>,
    /// CC recipients, up to 100 entries. An empty list overrides a batch default.
    pub cc: Option<Vec<MailAddress>>,
    /// BCC recipients, up to 100 entries. An empty list overrides a batch default.
    pub bcc: Option<Vec<MailAddress>>,
    /// Independent reply-to list, up to 10 entries.
    pub reply_to: Option<Vec<MailAddress>>,
    /// Optional plain-text body, which may be empty.
    pub text: Option<String>,
    /// At most 100 attachments. An empty list overrides a batch default.
    pub attachments: Option<Vec<Attachment>>,
    /// Context for server-side filtering, with a maximum of 1,000 Unicode characters.
    pub filter_context: Nullable<String>,
    /// Earliest permitted delivery time. `Null` sends an explicit JSON `null`.
    pub delay_until: Nullable<DelayUntil>,
}

impl SendMailOptions {
    /// Creates a message with the required fields and all optional fields omitted.
    pub fn new(to: Vec<MailAddress>, subject: impl Into<String>, html: impl Into<String>) -> Self {
        Self {
            to,
            subject: subject.into(),
            html: html.into(),
            idempotency_key: None,
            cc: None,
            bcc: None,
            reply_to: None,
            text: None,
            attachments: None,
            filter_context: Nullable::Missing,
            delay_until: Nullable::Missing,
        }
    }
}

/// Between 1 and 100 personalized messages with optional shared defaults.
#[derive(Debug, Clone)]
pub struct SendPersonalizedMailBatchOptions {
    /// Messages, in submission order.
    pub messages: Vec<SendMailOptions>,
    /// Shared attachments, applied to messages with omitted attachments.
    pub attachments: Option<Vec<Attachment>>,
    /// Shared filter context, applied to messages with omitted filter context.
    pub filter_context: Nullable<String>,
    /// Shared reply-to addresses, applied to messages with omitted reply-to addresses.
    pub reply_to: Option<Vec<MailAddress>>,
    /// Shared CC addresses, applied to messages with omitted CC addresses.
    pub cc: Option<Vec<MailAddress>>,
    /// Shared BCC addresses, applied to messages with omitted BCC addresses.
    pub bcc: Option<Vec<MailAddress>>,
}

impl SendPersonalizedMailBatchOptions {
    /// Creates a batch with no shared defaults.
    pub fn new(messages: Vec<SendMailOptions>) -> Self {
        Self {
            messages,
            attachments: None,
            filter_context: Nullable::Missing,
            reply_to: None,
            cc: None,
            bcc: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WireMail<'a> {
    to: Vec<MailAddress>,
    subject: &'a str,
    html: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cc: Option<Vec<MailAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bcc: Option<Vec<MailAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reply_to: Option<Vec<MailAddress>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(skip_serializing_if = "Nullable::is_missing")]
    filter_context: Nullable<String>,
    #[serde(skip_serializing_if = "Nullable::is_missing")]
    delay_until: Nullable<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<WireAttachment<'a>>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireAttachment<'a> {
    filename: &'a str,
    content_type: String,
    upload_index: usize,
}

#[derive(Serialize)]
pub(crate) struct WireBatch<'a> {
    messages: Vec<WireMail<'a>>,
}

#[derive(Default)]
struct BatchDefaults<'a> {
    cc: Option<&'a [MailAddress]>,
    bcc: Option<&'a [MailAddress]>,
    reply_to: Option<&'a [MailAddress]>,
    filter_context: Nullable<String>,
    attachments: Option<Vec<Upload<'a>>>,
}

pub(crate) async fn prepare_mail(
    options: &SendMailOptions,
) -> Result<(WireMail<'_>, Vec<Upload<'_>>), Error> {
    let mut uploads = Vec::new();
    let message = normalize_mail(options, &BatchDefaults::default(), &mut uploads).await?;
    Ok((message, uploads))
}

pub(crate) async fn prepare_batch(
    options: &SendPersonalizedMailBatchOptions,
) -> Result<(WireBatch<'_>, Vec<Upload<'_>>), Error> {
    if !(1..=100).contains(&options.messages.len()) {
        return Err(Error::input(
            "messages",
            "expected between 1 and 100 messages",
        ));
    }

    let defaults = BatchDefaults {
        attachments: load_optional_uploads(options.attachments.as_deref()).await?,
        filter_context: normalize_context(&options.filter_context)?,
        reply_to: options.reply_to.as_deref(),
        cc: options.cc.as_deref(),
        bcc: options.bcc.as_deref(),
    };
    for (field, values) in [
        ("replyTo", defaults.reply_to),
        ("cc", defaults.cc),
        ("bcc", defaults.bcc),
    ] {
        if let Some(values) = values {
            normalize_addresses(values, field, &mut HashSet::new())?;
        }
    }

    let mut uploads = Vec::new();
    let mut messages = Vec::with_capacity(options.messages.len());
    for message in &options.messages {
        messages.push(normalize_mail(message, &defaults, &mut uploads).await?);
    }
    Ok((WireBatch { messages }, uploads))
}

async fn normalize_mail<'a>(
    options: &'a SendMailOptions,
    defaults: &BatchDefaults<'a>,
    uploads: &mut Vec<Upload<'a>>,
) -> Result<WireMail<'a>, Error> {
    let mut seen = HashSet::new();
    let to = normalize_addresses(&options.to, "to", &mut seen)?;
    if to.is_empty() {
        return Err(Error::input("to", "a recipient is required"));
    }
    if options.subject.trim_matches(js_whitespace).is_empty()
        || options.subject.chars().count() > 5_000
        || options.subject.chars().any(char::is_control)
    {
        return Err(Error::input(
            "subject",
            "expected 1 to 5000 characters without controls",
        ));
    }
    if options.html.trim_matches(js_whitespace).is_empty() {
        return Err(Error::input(
            "html",
            "the HTML body must contain non-whitespace text",
        ));
    }
    if options.idempotency_key.as_deref().is_some_and(|key| {
        key.is_empty()
            || php_trim(key) != key
            || key.chars().count() > 255
            || key.chars().any(char::is_control)
    }) {
        return Err(Error::input(
            "idempotencyKey",
            "invalid length, surrounding whitespace, or controls",
        ));
    }

    let cc = normalize_optional_addresses(options.cc.as_deref().or(defaults.cc), "cc", &mut seen)?;
    let bcc =
        normalize_optional_addresses(options.bcc.as_deref().or(defaults.bcc), "bcc", &mut seen)?;
    let reply_to = normalize_optional_addresses(
        options.reply_to.as_deref().or(defaults.reply_to),
        "replyTo",
        &mut HashSet::new(),
    )?;
    let filter_context = if options.filter_context.is_missing() {
        defaults.filter_context.clone()
    } else {
        normalize_context(&options.filter_context)?
    };
    let delay_until = normalize_delay(&options.delay_until)?;

    let attachments = match options.attachments.as_deref() {
        Some(attachments) => Some(load_uploads(attachments).await?),
        None => defaults.attachments.clone(),
    };
    let attachments = attachments.map(|attachments| {
        attachments
            .into_iter()
            .map(|attachment| {
                let wire = WireAttachment {
                    filename: attachment.filename,
                    content_type: attachment.content_type.clone(),
                    upload_index: uploads.len(),
                };
                uploads.push(attachment);
                wire
            })
            .collect()
    });

    Ok(WireMail {
        to,
        subject: &options.subject,
        html: &options.html,
        idempotency_key: options.idempotency_key.as_deref(),
        cc,
        bcc,
        reply_to,
        text: options.text.as_deref(),
        filter_context,
        delay_until,
        attachments,
    })
}

async fn load_optional_uploads(
    input: Option<&[Attachment]>,
) -> Result<Option<Vec<Upload<'_>>>, Error> {
    match input {
        Some(attachments) => load_uploads(attachments).await.map(Some),
        None => Ok(None),
    }
}

fn normalize_optional_addresses(
    input: Option<&[MailAddress]>,
    field: &'static str,
    seen: &mut HashSet<String>,
) -> Result<Option<Vec<MailAddress>>, Error> {
    input
        .map(|addresses| normalize_addresses(addresses, field, seen))
        .transpose()
}

fn normalize_addresses(
    input: &[MailAddress],
    field: &'static str,
    seen: &mut HashSet<String>,
) -> Result<Vec<MailAddress>, Error> {
    let max = if field == "replyTo" { 10 } else { 100 };
    if input.len() > max {
        return Err(Error::input(field, "too many addresses"));
    }
    let mut output = Vec::with_capacity(input.len());
    for address in input {
        let trimmed = php_trim(&address.email);
        if address.email.chars().any(char::is_control) || !valid_email(trimmed) {
            return Err(Error::input(field, "invalid mailbox address"));
        }
        let email = trimmed.to_ascii_lowercase();
        let name = match &address.name {
            Nullable::Missing => Nullable::Missing,
            Nullable::Null => Nullable::Null,
            Nullable::Value(name) if name.chars().any(char::is_control) => {
                return Err(Error::input(field, "display name contains controls"));
            }
            Nullable::Value(name) => {
                let name = php_trim(name);
                if name.is_empty() {
                    Nullable::Null
                } else {
                    Nullable::Value(name.to_owned())
                }
            }
        };
        if seen.insert(email.clone()) {
            output.push(MailAddress { email, name });
        }
    }
    Ok(output)
}

fn valid_email(email: &str) -> bool {
    if email.len() > 254 || !email.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        return false;
    }
    let Some((local, domain)) = email.rsplit_once('@') else {
        return false;
    };
    if local.len() > 64 || !(valid_dot_atom(local) || valid_quoted_local(local)) {
        return false;
    }
    if let Some(address) = domain
        .strip_prefix('[')
        .and_then(|part| part.strip_suffix(']'))
    {
        let ip = if address
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("IPv6:"))
        {
            &address[5..]
        } else {
            address
        };
        if let Some((address, scope)) = ip.split_once('%') {
            return address.parse::<Ipv6Addr>().is_ok()
                && !scope.is_empty()
                && scope
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"-.:".contains(&byte));
        }
        return ip.parse::<IpAddr>().is_ok();
    }
    domain.len() <= 253
        && domain.contains('.')
        && domain.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

fn valid_dot_atom(local: &str) -> bool {
    local.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+/=?^_`{|}~-".contains(&byte))
    })
}

fn valid_quoted_local(local: &str) -> bool {
    let [b'"', inner @ .., b'"'] = local.as_bytes() else {
        return false;
    };
    let mut escaped = false;
    for &byte in inner {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            escaped = true;
        } else if byte == b'"' {
            return false;
        }
    }
    !escaped
}

fn normalize_context(input: &Nullable<String>) -> Result<Nullable<String>, Error> {
    let value = match input {
        Nullable::Missing => return Ok(Nullable::Missing),
        Nullable::Null => return Ok(Nullable::Null),
        Nullable::Value(value) => value.replace("\r\n", "\n").replace('\r', "\n"),
    };
    if value
        .chars()
        .any(|character| character.is_control() && !matches!(character, '\t' | '\n'))
    {
        return Err(Error::input(
            "filterContext",
            "context contains unsupported controls",
        ));
    }
    let trimmed = php_trim(&value);
    if trimmed.chars().count() > EmailApiClient::MAX_FILTER_CONTEXT_CHARACTERS {
        return Err(Error::input(
            "filterContext",
            "context exceeds 1000 Unicode characters",
        ));
    }
    Ok(if trimmed.is_empty() {
        Nullable::Null
    } else {
        Nullable::Value(trimmed.to_owned())
    })
}

fn normalize_delay(input: &Nullable<DelayUntil>) -> Result<Nullable<i64>, Error> {
    let invalid = || Error::input("delayUntil", "expected Epoch seconds in 1..=253402300799");
    let value = match input {
        Nullable::Missing => return Ok(Nullable::Missing),
        Nullable::Null => return Ok(Nullable::Null),
        Nullable::Value(DelayUntil::Seconds(value)) => *value,
        Nullable::Value(DelayUntil::Text(text)) => parse_positive(text, "delayUntil")?,
        Nullable::Value(DelayUntil::Time(value)) => {
            let duration = value.duration_since(UNIX_EPOCH).map_err(|_| invalid())?;
            i64::try_from(duration.as_secs()).map_err(|_| invalid())?
        }
    };
    if !(1..=MAX_DELAY_UNTIL).contains(&value) {
        return Err(invalid());
    }
    Ok(Nullable::Value(value))
}
