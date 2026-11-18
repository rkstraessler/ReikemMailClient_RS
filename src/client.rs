use std::fmt;
use std::path::Path;

use reqwest::header::CONTENT_TYPE;
use reqwest::multipart::{Form, Part};
use reqwest::{RequestBuilder, Url};
use serde::{Serialize, de::DeserializeOwned};

use crate::attachments::{Upload, create_attachments};
use crate::download;
use crate::history::HistoryRequest;
use crate::mail::{prepare_batch, prepare_mail};
use crate::response;
use crate::{
    ApiResponse, BatchResponse, BinaryAttachment, ClientConfig, DownloadFileOptions,
    DownloadOutcome, Error, GetMailHistoryOptions, SendMailOptions,
    SendPersonalizedMailBatchOptions, SendResponse,
};

/// An asynchronous client for the REIKEM Mail API v2.
///
/// Clients can be used concurrently on Tokio. Clones share the connection pool.
/// Dropping a request future cancels waiting for its HTTP operation. If the server
/// has already accepted a send, it can continue processing it.
#[derive(Clone)]
pub struct EmailApiClient {
    endpoint_root: Url,
    http: reqwest::Client,
}

impl EmailApiClient {
    /// Maximum length of a normalized filter context in Unicode characters.
    pub const MAX_FILTER_CONTEXT_CHARACTERS: usize = 1_000;

    /// Creates the pooled Rustls HTTP client with the supplied configuration.
    ///
    /// # Errors
    /// Returns an error if the HTTP client could not be created.
    pub fn new(config: ClientConfig) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .tls_backend_rustls()
            .http1_only()
            .default_headers(config.headers)
            .connect_timeout(config.connect_timeout)
            .timeout(config.request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy()
            .no_gzip()
            .no_brotli()
            .no_deflate()
            .no_zstd()
            .build()
            .map_err(|_| Error::ClientInitialization)?;
        Ok(Self {
            endpoint_root: config.endpoint_root,
            http,
        })
    }

    /// Creates a client using the default connect and request timeouts.
    ///
    /// # Errors
    /// Returns an error if the configuration is invalid or the client cannot be built.
    pub fn from_credentials(
        base_url: &str,
        auth_key: &str,
        adapter_key: &str,
    ) -> Result<Self, Error> {
        Self::new(ClientConfig::new(base_url, auth_key, adapter_key)?)
    }

    /// Submits one mail, grouping all To addresses under one send operation.
    ///
    /// # Errors
    /// Fails on invalid input, unreadable attachments, transport errors, or malformed
    /// API envelopes. A valid non-2xx envelope is returned with its HTTP status.
    pub async fn send_mail(&self, options: &SendMailOptions) -> Result<SendResponse, Error> {
        let (mail, uploads) = prepare_mail(options).await?;
        self.send_json_response(encode_mail(self.post("send")?, &mail, uploads)?)
            .await
    }

    /// Submits one mail with the same behavior as [`Self::send_mail`].
    ///
    /// # Errors
    /// Returns the same errors as [`Self::send_mail`].
    pub async fn send_mail_list(&self, options: &SendMailOptions) -> Result<SendResponse, Error> {
        self.send_mail(options).await
    }

    /// Submits a batch after expanding shared defaults into individual messages.
    ///
    /// # Errors
    /// Fails on invalid input, unreadable attachments, transport errors, or malformed
    /// API envelopes. Individual message failures can appear in a valid batch response.
    pub async fn send_personalized_mail_batch(
        &self,
        options: &SendPersonalizedMailBatchOptions,
    ) -> Result<BatchResponse, Error> {
        let (batch, uploads) = prepare_batch(options).await?;
        self.send_json_response(encode_mail(self.post("send-batch")?, &batch, uploads)?)
            .await
    }

    /// Queries mail history using a date range, message IDs, or both.
    ///
    /// # Errors
    /// Fails on invalid filters, transport errors, or malformed API envelopes.
    pub async fn get_mail_history(
        &self,
        options: &GetMailHistoryOptions,
    ) -> Result<ApiResponse, Error> {
        let request = encode_json(self.post("history")?, &HistoryRequest::try_from(options)?)?;
        self.send_json_response(request).await
    }

    /// Downloads a file into memory or saves it to a caller-selected path.
    ///
    /// # Errors
    /// Fails on invalid input, transport or file errors, malformed error envelopes,
    /// or a successful HTTP response that contains an API error instead of a file.
    pub async fn download_file(
        &self,
        options: &DownloadFileOptions,
    ) -> Result<DownloadOutcome, Error> {
        let payload = download::DownloadRequest::try_from(options)?;
        let request = encode_json(self.post("files/download")?, &payload)?;
        let response = request.send().await.map_err(Error::transport)?;
        let http_code = response.status().as_u16();
        if !response.status().is_success() {
            let body = response.bytes().await.map_err(Error::transport)?;
            return response::decode(&body, http_code)
                .map(Box::new)
                .map(DownloadOutcome::ApiError);
        }

        let metadata = download::DownloadMetadata::from_headers(response.headers());
        let content = response.bytes().await.map_err(Error::transport)?;
        download::finish_download(options, metadata, content, http_code)
            .await
            .map(DownloadOutcome::File)
    }

    /// Loads local files in the same way as [`create_attachments`].
    ///
    /// # Errors
    /// Returns the same errors as [`create_attachments`].
    pub async fn create_attachments<P: AsRef<Path>>(
        &self,
        paths: &[P],
    ) -> Result<Vec<BinaryAttachment>, Error> {
        create_attachments(paths).await
    }

    fn post(&self, endpoint: &str) -> Result<RequestBuilder, Error> {
        let url = self
            .endpoint_root
            .join(endpoint)
            .map_err(|_| Error::InvalidBaseUrl)?;
        Ok(self.http.post(url))
    }

    async fn send_json_response<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
    ) -> Result<ApiResponse<T>, Error> {
        let response = request.send().await.map_err(Error::transport)?;
        let http_code = response.status().as_u16();
        let body = response.bytes().await.map_err(Error::transport)?;
        response::decode(&body, http_code)
    }
}

impl fmt::Debug for EmailApiClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EmailApiClient")
            .finish_non_exhaustive()
    }
}

fn encode_json(request: RequestBuilder, payload: &impl Serialize) -> Result<RequestBuilder, Error> {
    let bytes = serde_json::to_vec(payload).map_err(|_| Error::RequestEncoding)?;
    Ok(request.header(CONTENT_TYPE, "application/json").body(bytes))
}

fn encode_mail(
    request: RequestBuilder,
    payload: &impl Serialize,
    uploads: Vec<Upload<'_>>,
) -> Result<RequestBuilder, Error> {
    if uploads.is_empty() {
        return encode_json(request, payload);
    }
    let json = serde_json::to_string(payload).map_err(|_| Error::RequestEncoding)?;
    // The brackets are part of the form field names, and upload names match the reference's quoting.
    let mut form = Form::new().percent_encode_noop().text("payload", json);
    for (index, upload) in uploads.into_iter().enumerate() {
        let Upload {
            filename,
            content_type,
            bytes,
        } = upload;
        let length = bytes.len() as u64;
        let part = Part::stream_with_length(bytes, length)
            .file_name(filename.replace('"', "%22"))
            .mime_str(&content_type)
            .map_err(|_| Error::RequestEncoding)?;
        form = form.part(format!("attachments[{index}]"), part);
    }
    Ok(request.multipart(form))
}
