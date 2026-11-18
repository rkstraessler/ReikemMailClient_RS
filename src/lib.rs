//! Asynchronous client for the REIKEM Mail API v2.
//!
//! A client shares a pooled Rustls HTTP transport with its clones and runs on
//! Tokio. The API's HTTP errors are returned in response envelopes. Local
//! validation, transport and malformed responses use [`Error`].
//!
//! ```no_run
//! use reikem_mail_client::{EmailApiClient, MailAddress, SendMailOptions};
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let client = EmailApiClient::from_credentials(
//!     "https://mail.example.com/proxy",
//!     &std::env::var("MAIL_AUTH_KEY")?,
//!     &std::env::var("MAIL_ADAPTER_KEY")?,
//! )?;
//! let message = SendMailOptions::new(
//!     vec![MailAddress::new("recipient@example.com")],
//!     "Welcome",
//!     "<p>Hello!</p>",
//! );
//! let result = client.send_mail(&message).await?;
//! assert!(result.is_http_success(), "HTTP {}", result.http_code);
//! # Ok(())
//! # }
//! ```

mod attachments;
mod client;
mod config;
mod download;
mod error;
mod history;
mod mail;
mod response;
mod validation;

pub use attachments::{Attachment, BinaryAttachment, EmailAttachment, create_attachments};
pub use client::EmailApiClient;
pub use config::ClientConfig;
pub use download::{
    DownloadFileOptions, DownloadOutcome, DownloadResponse, DownloadSource, DownloadTarget,
    DownloadedFile,
};
pub use error::{Error, TransportErrorKind};
pub use history::{ApiId, GetMailHistoryOptions, HistoryDate, HistoryMessageIds};
pub use mail::{DelayUntil, MailAddress, SendMailOptions, SendPersonalizedMailBatchOptions};
pub use response::{
    AiFilterDetails, AiResult, AiRetry, ApiData, ApiInteger, ApiPayload, ApiResponse, BatchData,
    BatchResponse, BatchResult, FilterDetails, FilterResults, HistoryResults, MailEntry,
    MailHistory, MailStatus, ObjectOrArray, QueueStatus, Rejection, SendData, SendResponse,
};
pub use validation::Nullable;
