# REIKEM Mail Client for Rust

[![CI](https://github.com/rkstraessler/ReikemMailClient_RS/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/rkstraessler/ReikemMailClient_RS/actions/workflows/ci.yml)
[![MIT license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

An asynchronous Rust SDK for the REIKEM Mail API v2. Send grouped or personalized mail, look up delivery history, and download attachments through a shared Reqwest connection pool with Rustls and platform certificate verification.

## Quick start

The SDK requires Rust 1.88 or later and a Tokio runtime. Reqwest's Rustls provider uses a native C toolchain when it builds from source. On Windows, use the MSVC Build Tools and Windows SDK.

```toml
[dependencies]
reikem-mail-client = { git = "https://github.com/rkstraessler/ReikemMailClient_RS", tag = "v0.1.0" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

For development snapshots, pin a commit with `rev`. [GitHub Releases](https://github.com/rkstraessler/ReikemMailClient_RS/releases)

```rust,no_run
use std::{env, time::Duration};

use reikem_mail_client::{ClientConfig, EmailApiClient, MailAddress, SendMailOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ClientConfig::new(
        &env::var("REIKEM_MAIL_URL")?,
        &env::var("REIKEM_MAIL_AUTH_KEY")?,
        &env::var("REIKEM_MAIL_ADAPTER_KEY")?,
    )?
    .with_timeouts(Duration::from_secs(10), Duration::from_secs(30))?;
    let client = EmailApiClient::new(config)?;

    let mut mail = SendMailOptions::new(
        vec![MailAddress::new("recipient@example.com")],
        "Your account is ready",
        "<p>Welcome!</p>",
    );
    mail.idempotency_key = Some("welcome:account-123:v1".into());

    let response = client.send_mail(&mail).await?;
    if response.is_idempotency_conflict() {
        eprintln!("This key was already used for different mail content.");
    } else if !response.is_http_success() {
        eprintln!("The API returned HTTP {}.", response.http_code);
    } else if let Some(data) = response.data.as_object() {
        println!("Queue IDs: {:?}", data.queue_ids);
    }
    Ok(())
}
```

Use an HTTPS origin with an optional reverse-proxy prefix. The client also accepts `/api/send`, `/api/v2/send`, and `/api/v2` URL suffixes. It sends `Authorization: Bearer <authKey>` and `X-Adapter-Key: <adapterKey>` with every request. Both keys accept printable non-space ASCII. The connection timeout covers TCP and TLS. The request timeout covers the HTTP exchange through the response body. Both timeouts take whole seconds between 1 and 2,147,483.

## TypeScript and Rust feature matrix

The TypeScript package exports the following operations. The Rust calls use the same POST routes and wire payloads.

| TypeScript | Rust | Request / result |
| --- | --- | --- |
| `new EmailAPIClient(options)` | `EmailApiClient::new(ClientConfig::new(…)?)` | Configure base URL, both headers and timeouts |
| `new EmailAPIClient(url, authKey, adapterKey)` | `EmailApiClient::from_credentials` | Same configuration with the default timeouts of 10 and 30 seconds |
| `sendMail(options)` | `send_mail(&SendMailOptions)` | `/api/v2/send` → `SendResponse` |
| `sendMailList(options)` | `send_mail_list(&SendMailOptions)` | Alias for grouped send through `/api/v2/send` |
| `sendPersonalizedMailBatch(options)` | `send_personalized_mail_batch(&SendPersonalizedMailBatchOptions)` | `/api/v2/send-batch` → `BatchResponse` with ordered partial results |
| `getMailHistory(options)` | `get_mail_history(&GetMailHistoryOptions)` | `/api/v2/history` → `ApiResponse<ApiData>` with inbound and outbound history |
| `downloadFile(options)` | `download_file(&DownloadFileOptions)` | `/api/v2/files/download` → `DownloadOutcome` with bytes, a saved path, or an API error envelope |
| `client.createAttachments(paths)` / `createAttachments(paths)` | `client.create_attachments(&paths)` / `create_attachments(&paths)` | Read regular files and resolve their MIME types using the same extension data |
| `new EmailAttachment(filename, mimeType, tmpPath, size)` | `EmailAttachment::new` and `Attachment::from` | Read the file when sending and use its actual length |
| `MailStatus` constants, `all()`, `isKnown()` | `MailStatus` enum, `all()`, `is_known()` | All eight wire statuses. `QueueStatus::Other` preserves new values |
| `EmailAPIClient.MAX_FILTER_CONTEXT_CHARACTERS` | `EmailApiClient::MAX_FILTER_CONTEXT_CHARACTERS` | 1,000 Unicode characters after normalization |
| `EmailAPITransportError` | `Error::Transport(TransportErrorKind)` | Separate timeout, connection, and body-transfer failures |

### Send and batch options

- To, CC and BCC take up to 100 addresses each. To must have a recipient. Deduplication keeps the first match in To → CC → BCC order. Reply-To takes up to 10 addresses and deduplicates independently. One grouped send can include several To addresses.
- The SDK preserves subject, HTML and plain-text contents after validation. Filter context normalizes CRLF and CR to LF and trims PHP whitespace. `DelayUntil` accepts positive Unix seconds through `253402300799` or a `SystemTime` rounded down to whole seconds.
- Batches accept 1–100 messages and expand `cc`, `bcc`, `reply_to`, `attachments` and `filter_context` defaults. An omitted value inherits its default. An empty list or explicit `Nullable::Null` overrides it where the field permits that value.
- Without file parts, the SDK sends JSON. For uploads, `payload` holds the JSON and `attachments[n]` holds the original bytes. Each message accepts up to 100 unique file names. Multipart indices are continuous across the batch. `BinaryAttachment::new` accepts bytes directly.
- An `idempotency_key` is passed unchanged to the server. The API can return a previous send snapshot with `replayed: true` or HTTP 409 with `idempotency_conflict`. To check the current delivery state, query history.

```rust
use reikem_mail_client::{EmailApiClient, Error, GetMailHistoryOptions, HistoryMessageIds};

async fn count_outbound(client: &EmailApiClient) -> Result<usize, Error> {
    let response = client.get_mail_history(&GetMailHistoryOptions {
        date_from: Some("2026-11-01".into()),
        message_ids: Some(HistoryMessageIds::Csv("123, 00456, 123".into())),
        ..Default::default()
    }).await?;
    Ok(response.data.as_object()
        .and_then(|data| data.history.as_ref())
        .and_then(|history| history.outbound.as_deref())
        .map_or(0, |messages| messages.len()))
}
```

### History and downloads

History accepts `YYYY-MM-DD` strings or UTC dates derived from `SystemTime`. Supply `date_from` when querying without IDs. History and download IDs accept positive `i64` values or decimal strings and are sent as canonical decimal strings. The reference API exposes no pagination parameters.

`DownloadFileOptions::new(DownloadSource::Inbound, "123")` selects memory. Choose `DownloadTarget::Directory(path)` to save under the server's sanitized name or `DownloadTarget::Path(path)` to select a file. An existing directory or trailing separator uses the server name. A download validates the file response before replacing a destination. Use caller-controlled paths for local files and destinations. The SDK holds attachment bytes and responses in memory.

### Response and error handling

Valid API envelopes are returned as values at any HTTP status. `http_code` is the actual HTTP status. Typed payloads are available through `data.as_object()`. Unknown fields, new queue statuses, unexpected object schemas, and array envelopes are preserved. Numbers outside JavaScript's safe integer range retain their original JSON spelling as strings. JSON responses are limited to 128 levels of nesting.

`Error` reports input, transport, invalid JSON, invalid envelopes, and local file failures without embedding credentials or response bodies. The client follows no redirects and performs no automatic retries. Dropping the future stops waiting for the HTTP request. A submitted send may still be processed by the server, and an already started file replacement may finish.

## Development

Run `cargo test --locked --all-features` for the local HTTP and filesystem contract suite. Generate public API documentation with `cargo doc --no-deps --open`. GitHub Actions runs formatting, build, Clippy, Rust 1.88 compatibility, dependency review and RustSec checks.

## GitHub maintenance

- `main` requires a pull request, one current approval, resolved conversations and the GitHub Actions `Required CI` check on an up-to-date branch. Only repository administrators have a bypass. CODEOWNERS requests the maintainer's review. The repository uses squash merges and deletes merged branches. [Ruleset definition](.github/main-ruleset.json)
- Dependabot checks Cargo and GitHub Actions daily at 03:00 Europe/Berlin. Compatible, verified patch and minor updates can use native auto-merge after CI and review. Major updates, breaking pre-1.0 updates, conflicting changes, and branches with extra authors require manual review. Run `bash .github/scripts/sync-labels.sh` to synchronize the PR labels.
- autofix.ci applies rustfmt and machine-applicable Clippy fixes on pull requests. Install the official app for this repository. Its fixes trigger CI on the new revision. [autofix.ci](https://autofix.ci/)
- The PR Maintenance app updates open PR branches against their target branches when GitHub permits it. It can approve eligible Dependabot revisions once their CI passes. Configure `PR_AUTOMATION_CLIENT_ID` and the encrypted `PR_AUTOMATION_PRIVATE_KEY` in the `dependabot-maintenance` environment, restricted to `main`. The workflow requests a repository-scoped installation token with Contents, Pull requests, and Workflows write access. It has no ruleset bypass. Without the app, Dependabot retains native rebasing and a maintainer supplies the approval. [GitHub App token action](https://github.com/actions/create-github-app-token)

## Releases

1. Update `Cargo.toml` and `Cargo.lock` for the next SemVer version. Wait for `Required CI` on `main`.
2. Tag the checked commit as `vMAJOR.MINOR.PATCH`. Run **Verify release** on `main` with that tag to check the version, ancestry, tests and Cargo package.
3. Create a GitHub Release draft from the existing tag. Use the verification run's banner Markdown and the release categories from [release.yml](.github/release.yml). Review and publish the Release Notes on GitHub.

The GitHub tag is the Cargo dependency source. Registry publication requires separate approval. [Release banner](.github/assets/release-banners/reikem-mail-client.png) · [MIT license](LICENSE)
