use std::borrow::Cow;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::{fmt, fs, str::FromStr};

use bytes::Bytes;
use percent_encoding::percent_decode_str;
use reqwest::header::{CONTENT_DISPOSITION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde::Serialize;

use crate::response::looks_like_envelope;
use crate::validation::js_whitespace;
use crate::{ApiId, ApiResponse, Error};

/// The direction to which an attachment belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadSource {
    /// Attachments received by the API.
    Inbound,
    /// Attachments sent through the API.
    Outbound,
}

impl FromStr for DownloadSource {
    type Err = Error;

    fn from_str(source: &str) -> Result<Self, Error> {
        let source = source.trim_matches(js_whitespace);
        if source.eq_ignore_ascii_case("inbound") {
            Ok(Self::Inbound)
        } else if source.eq_ignore_ascii_case("outbound") {
            Ok(Self::Outbound)
        } else {
            Err(Error::input("source", "expected inbound or outbound"))
        }
    }
}

/// Where the SDK delivers the downloaded file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum DownloadTarget {
    /// Return the file bytes to the caller.
    #[default]
    Memory,
    /// Use the supplied path. A directory or trailing separator uses the server file name.
    Path(PathBuf),
    /// Use the server file name in this directory, even if the directory does not exist yet.
    Directory(PathBuf),
}

/// Options for downloading one inbound or outbound file.
#[derive(Debug, Clone)]
pub struct DownloadFileOptions {
    /// Message direction.
    pub source: DownloadSource,
    /// Positive signed 64-bit file ID.
    pub file_id: ApiId,
    /// Destination. Defaults to returning the file bytes in memory.
    pub target: DownloadTarget,
}

impl DownloadFileOptions {
    /// Creates a download request with an in-memory destination.
    pub fn new(source: DownloadSource, file_id: impl Into<ApiId>) -> Self {
        Self {
            source,
            file_id: file_id.into(),
            target: DownloadTarget::Memory,
        }
    }
}

/// The successful result of a file download.
#[derive(Debug, Clone)]
pub struct DownloadResponse {
    /// Actual HTTP status of the file response.
    pub http_code: u16,
    /// Server file name with path components removed, or `download` as the fallback.
    pub file_name: String,
    /// Content-Type header, if present.
    pub content_type: Option<String>,
    /// Number of bytes downloaded.
    pub bytes: usize,
    /// The saved path or in-memory contents.
    pub file: DownloadedFile,
}

/// The materialized file from a successful download.
#[derive(Clone, PartialEq, Eq)]
pub enum DownloadedFile {
    /// Downloaded bytes in memory.
    Content(Bytes),
    /// Path where the file was saved.
    Saved(PathBuf),
}

impl fmt::Debug for DownloadedFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Content(bytes) => formatter
                .debug_tuple("Content")
                .field(&bytes.len())
                .finish(),
            Self::Saved(_) => formatter.debug_struct("Saved").finish_non_exhaustive(),
        }
    }
}

/// A file response or a valid non-2xx API envelope.
#[derive(Debug, Clone)]
pub enum DownloadOutcome {
    /// Successfully downloaded a file.
    File(DownloadResponse),
    /// The API rejected the download and returned an error envelope.
    ApiError(Box<ApiResponse>),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DownloadRequest {
    source: DownloadSource,
    file_id: String,
}

impl TryFrom<&DownloadFileOptions> for DownloadRequest {
    type Error = Error;

    fn try_from(options: &DownloadFileOptions) -> Result<Self, Error> {
        if let DownloadTarget::Path(path) | DownloadTarget::Directory(path) = &options.target {
            let path = path.as_os_str().as_encoded_bytes();
            if path.is_empty() || path.contains(&0) {
                return Err(Error::input(
                    "targetPath",
                    "expected a nonempty path without NUL",
                ));
            }
        }
        Ok(Self {
            source: options.source,
            file_id: options.file_id.positive_value("fileId")?.to_string(),
        })
    }
}

pub(crate) struct DownloadMetadata {
    file_name: String,
    content_type: Option<String>,
}

impl DownloadMetadata {
    pub(crate) fn from_headers(headers: &HeaderMap) -> Self {
        let file_name = headers
            .get(CONTENT_DISPOSITION)
            .and_then(|value| file_name_from_disposition(&header_text(value)))
            .unwrap_or_else(|| "download".to_owned());
        let content_type = headers
            .get(CONTENT_TYPE)
            .map(|value| header_text(value).into_owned());
        Self {
            file_name,
            content_type,
        }
    }
}

pub(crate) async fn finish_download(
    options: &DownloadFileOptions,
    metadata: DownloadMetadata,
    content: Bytes,
    http_code: u16,
) -> Result<DownloadResponse, Error> {
    if metadata
        .content_type
        .as_deref()
        .is_some_and(json_content_type)
        || json_file_body(&content)
    {
        return Err(Error::JsonInsteadOfFile { http_code });
    }

    let DownloadMetadata {
        file_name,
        content_type,
    } = metadata;
    let bytes = content.len();
    let file = match &options.target {
        DownloadTarget::Memory => DownloadedFile::Content(content),
        DownloadTarget::Path(path) | DownloadTarget::Directory(path) => {
            let path = path.clone();
            let name = file_name.clone();
            let directory = matches!(&options.target, DownloadTarget::Directory(_));
            let saved =
                tokio::task::spawn_blocking(move || save_file(path, &name, directory, &content))
                    .await
                    .map_err(|_| Error::DownloadWrite(io::ErrorKind::Other))?
                    .map_err(|error| Error::DownloadWrite(error.kind()))?;
            DownloadedFile::Saved(saved)
        }
    };
    Ok(DownloadResponse {
        http_code,
        file_name,
        content_type,
        bytes,
        file,
    })
}

fn save_file(
    mut path: PathBuf,
    name: &str,
    directory: bool,
    content: &[u8],
) -> io::Result<PathBuf> {
    if directory
        || fs::metadata(&path).is_ok_and(|metadata| metadata.is_dir())
        || matches!(
            path.as_os_str().as_encoded_bytes().last(),
            Some(b'/' | b'\\')
        )
    {
        path.push(name);
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".reikem-mail-")
        .tempfile_in(parent)?;
    temporary.write_all(content)?;
    temporary.flush()?;
    temporary.persist(&path).map_err(|error| error.error)?;
    Ok(path)
}

fn header_text(value: &HeaderValue) -> Cow<'_, str> {
    match std::str::from_utf8(value.as_bytes()) {
        Ok(text) => Cow::Borrowed(text),
        Err(_) => Cow::Owned(value.as_bytes().iter().copied().map(char::from).collect()),
    }
}

fn file_name_from_disposition(value: &str) -> Option<String> {
    let extended = after_case_insensitive(value, "filename*=UTF-8''")
        .and_then(|value| value.split(';').next())
        .filter(|value| !value.is_empty());
    let name = if let Some(extended) = extended {
        decode_extended_filename(extended.trim_matches(js_whitespace))?
    } else {
        let quoted = after_case_insensitive(value, "filename=\"")
            .and_then(|tail| tail.split_once('"'))
            .map(|(name, _)| name)
            .filter(|name| !name.is_empty());
        let legacy = quoted.or_else(|| {
            after_case_insensitive(value, "filename=").and_then(|tail| tail.split(';').next())
        })?;
        Cow::Borrowed(legacy)
    };
    if name
        .chars()
        .any(|character| character.is_control() || character == ':')
    {
        return None;
    }
    let base = name
        .trim_matches(js_whitespace)
        .rsplit(['/', '\\'])
        .next()?;
    (!matches!(base, "" | "." | "..")).then(|| base.to_owned())
}

fn after_case_insensitive<'a>(text: &'a str, needle: &str) -> Option<&'a str> {
    let start = text
        .as_bytes()
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle.as_bytes()))?;
    text.get(start + needle.len()..)
}

fn decode_extended_filename(value: &str) -> Option<Cow<'_, str>> {
    let bytes = value.as_bytes();
    for (index, _) in bytes.iter().enumerate().filter(|(_, byte)| **byte == b'%') {
        if !bytes
            .get(index + 1..index + 3)
            .is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit))
        {
            return None;
        }
    }
    percent_decode_str(value).decode_utf8().ok()
}

fn json_content_type(value: &str) -> bool {
    let media_type = value
        .split(';')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    media_type.ends_with("application/json") || media_type.ends_with("+json")
}

fn json_file_body(content: &[u8]) -> bool {
    // Avoid decoding arbitrary binary files just to check their first character.
    let prefix = content
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace());
    let could_be_json = matches!(
        prefix,
        Some(b'{' | b'[' | 0x0B | 0xC2 | 0xE1 | 0xE2 | 0xE3 | 0xEF)
    );
    if !could_be_json {
        return false;
    }
    let decoded = String::from_utf8_lossy(content);
    let trimmed = decoded.trim_start_matches(js_whitespace);
    trimmed.starts_with(['{', '[']) && looks_like_envelope(trimmed.as_bytes())
}
