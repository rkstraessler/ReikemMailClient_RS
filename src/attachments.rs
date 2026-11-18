use std::collections::HashSet;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use bytes::Bytes;
use tokio::fs;

use crate::Error;
use crate::validation::js_whitespace;

pub(crate) const MAX_ATTACHMENTS: usize = 100;
const MIME_TYPES: &[(&str, &str)] = include!("mime_types.in");

/// A binary attachment held in memory. Clones share the underlying bytes.
#[derive(Clone, Debug)]
pub struct BinaryAttachment {
    /// File name supplied to the API.
    pub filename: String,
    /// Media type in `type/subtype` form without parameters.
    pub content_type: String,
    /// File bytes.
    pub content: Bytes,
}

impl BinaryAttachment {
    /// Creates an attachment from bytes. Its name and media type are validated when sent.
    pub fn new(
        filename: impl Into<String>,
        content_type: impl Into<String>,
        content: impl Into<Bytes>,
    ) -> Self {
        Self {
            filename: filename.into(),
            content_type: content_type.into(),
            content: content.into(),
        }
    }
}

/// A file-backed attachment compatible with the TypeScript `EmailAttachment` adapter.
#[derive(Clone, Debug)]
pub struct EmailAttachment {
    /// File name supplied to the API.
    pub filename: String,
    /// Media type in `type/subtype` form without parameters.
    pub mime_type: String,
    /// Caller-selected regular file to read when sending.
    pub tmp_path: PathBuf,
    /// Legacy metadata. The upload length is calculated from the actual bytes.
    pub size: u64,
}

impl EmailAttachment {
    /// Creates a descriptor without opening the file.
    pub fn new(
        filename: impl Into<String>,
        mime_type: impl Into<String>,
        tmp_path: impl Into<PathBuf>,
        size: u64,
    ) -> Self {
        Self {
            filename: filename.into(),
            mime_type: mime_type.into(),
            tmp_path: tmp_path.into(),
            size,
        }
    }
}

/// An attachment supplied as bytes or as a file descriptor.
#[derive(Clone, Debug)]
pub enum Attachment {
    /// Already loaded file bytes.
    Binary(BinaryAttachment),
    /// A regular file read when preparing the send.
    File(EmailAttachment),
}

impl From<BinaryAttachment> for Attachment {
    fn from(value: BinaryAttachment) -> Self {
        Self::Binary(value)
    }
}

impl From<EmailAttachment> for Attachment {
    fn from(value: EmailAttachment) -> Self {
        Self::File(value)
    }
}

/// Loads local files as binary attachments, with the reference SDK's MIME lookup.
/// Pass the returned attachments to a send with `.map(Attachment::from)` on the iterator.
///
/// # Errors
/// Fails for more than 100 files, duplicate or invalid names, or unreadable files.
pub async fn create_attachments<P: AsRef<Path>>(
    paths: &[P],
) -> Result<Vec<BinaryAttachment>, Error> {
    validate_count(paths.len())?;
    let mut attachments = Vec::with_capacity(paths.len());
    let mut filenames = HashSet::new();

    for path in paths {
        let path = path.as_ref();
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::input("attachment.filename", "expected a UTF-8 file name"))?;
        let mime_type = guess_mime_type(path, filename);
        validate_metadata(filename, mime_type, &mut filenames)?;
        let content = read_attachment(path).await?;
        attachments.push(BinaryAttachment::new(filename, mime_type, content));
    }

    Ok(attachments)
}

#[derive(Clone)]
pub(crate) struct Upload<'a> {
    pub(crate) filename: &'a str,
    pub(crate) content_type: String,
    pub(crate) bytes: Bytes,
}

pub(crate) async fn load_uploads(attachments: &[Attachment]) -> Result<Vec<Upload<'_>>, Error> {
    validate_count(attachments.len())?;
    let mut uploads = Vec::with_capacity(attachments.len());
    let mut filenames = HashSet::new();

    for attachment in attachments {
        let (filename, content_type) = match attachment {
            Attachment::Binary(value) => (value.filename.as_str(), value.content_type.as_str()),
            Attachment::File(value) => (value.filename.as_str(), value.mime_type.as_str()),
        };
        validate_metadata(filename, content_type, &mut filenames)?;
        let bytes = match attachment {
            Attachment::Binary(value) => value.content.clone(),
            Attachment::File(value) => read_attachment(&value.tmp_path).await?,
        };
        uploads.push(Upload {
            filename,
            content_type: content_type.to_ascii_lowercase(),
            bytes,
        });
    }
    Ok(uploads)
}

fn validate_count(length: usize) -> Result<(), Error> {
    if length > MAX_ATTACHMENTS {
        return Err(Error::input(
            "attachments",
            "at most 100 entries are allowed",
        ));
    }
    Ok(())
}

fn validate_metadata<'a>(
    filename: &'a str,
    content_type: &str,
    filenames: &mut HashSet<&'a str>,
) -> Result<(), Error> {
    if matches!(filename.trim_matches(js_whitespace), "" | "." | "..")
        || filename.chars().count() > 500
        || filename
            .chars()
            .any(|character| character.is_control() || matches!(character, '\\' | '/' | ':'))
    {
        return Err(Error::input("attachment.filename", "invalid file name"));
    }
    if !filenames.insert(filename) {
        return Err(Error::input(
            "attachments",
            "file names must be unique per message",
        ));
    }
    if !content_type
        .split_once('/')
        .is_some_and(|(kind, subtype)| valid_media_token(kind) && valid_media_token(subtype))
    {
        return Err(Error::input(
            "attachment.contentType",
            "expected a bare media type",
        ));
    }
    Ok(())
}

fn valid_media_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_!#$&^.+-".contains(&byte))
}

fn guess_mime_type(path: &Path, filename: &str) -> &'static str {
    // mime-types treats a bare file name as a possible extension too.
    let extension = if path.as_os_str() == filename {
        filename
            .rsplit_once('.')
            .map_or(filename, |(_, extension)| extension)
    } else {
        path.extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
    };
    let extension = extension.to_ascii_lowercase();
    MIME_TYPES
        .binary_search_by_key(&extension.as_str(), |(extension, _)| *extension)
        .map_or("application/octet-stream", |index| MIME_TYPES[index].1)
}

async fn read_attachment(path: &Path) -> Result<Bytes, Error> {
    let metadata = fs::metadata(path)
        .await
        .map_err(|error| Error::AttachmentRead(error.kind()))?;
    if !metadata.is_file() {
        return Err(Error::AttachmentRead(ErrorKind::InvalidInput));
    }
    fs::read(path)
        .await
        .map(Bytes::from)
        .map_err(|error| Error::AttachmentRead(error.kind()))
}
