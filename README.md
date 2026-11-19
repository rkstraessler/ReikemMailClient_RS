# REIKEM Mail Client for Rust

[![CI](https://github.com/rkstraessler/ReikemMailClient_RS/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/rkstraessler/ReikemMailClient_RS/actions/workflows/ci.yml)

Rust SDK for the REIKEM Mail API v2. Send email, submit personalized batches, query mail history, and download files. The asynchronous client uses a shared Reqwest connection pool with Rustls and platform certificate verification.

[GitHub Releases](https://github.com/rkstraessler/ReikemMailClient_RS/releases) · [Report an issue](https://github.com/rkstraessler/ReikemMailClient_RS/issues/new)

Also available for [TypeScript](https://github.com/rkstraessler/ReikemMailClient_TS) and [Go](https://github.com/rkstraessler/ReikemMailClient_GO).

## Installation

The SDK requires Rust 1.88 or later and a Tokio runtime. Reqwest's Rustls provider uses a native C toolchain when it builds from source. On Windows, use the MSVC Build Tools and Windows SDK.

```toml
[dependencies]
reikem-mail-client = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

Registry installation is available once the first crate version is published. Until then, use the GitHub release with `reikem-mail-client = { git = "https://github.com/rkstraessler/ReikemMailClient_RS", tag = "v0.1.0" }`.

## Quick start

Set `MAIL_API_URL`, `MAIL_AUTH_KEY`, and `MAIL_ADAPTER_KEY` in your environment. The example queues one email and prints the queue IDs.

```rust,no_run
use std::{env, time::Duration};

use reikem_mail_client::{ClientConfig, EmailApiClient, MailAddress, SendMailOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = ClientConfig::new(
        &env::var("MAIL_API_URL")?,
        &env::var("MAIL_AUTH_KEY")?,
        &env::var("MAIL_ADAPTER_KEY")?,
    )?
    .with_timeouts(Duration::from_secs(10), Duration::from_secs(30))?;
    let client = EmailApiClient::new(config)?;

    let mut mail = SendMailOptions::new(
        vec![MailAddress::new("customer@example.com")],
        "Your contract is ready",
        "<p>Your contract is ready to review.</p>",
    );
    mail.idempotency_key = Some("contract:4711:v2:ready".into());

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

### Configuration

Use an HTTPS origin with an optional reverse-proxy prefix. The client also accepts `/api/send`, `/api/v2/send`, and `/api/v2` URL suffixes. It sends `Authorization: Bearer <authKey>` and `X-Adapter-Key: <adapterKey>` with every request. Both keys accept printable non-space ASCII. The connection timeout covers TCP and TLS. The request timeout covers the HTTP exchange through the response body. Both timeouts take whole seconds between 1 and 2,147,483.

## API overview

All network methods use `POST` with paths relative to the proxy prefix.

| Method | Endpoint | Use |
| --- | --- | --- |
| `send_mail` and `send_mail_list` | `/api/v2/send` | Queue one message for one or more To recipients. |
| `send_personalized_mail_batch` | `/api/v2/send-batch` | Submit up to 100 messages with shared defaults and ordered per-message results. |
| `get_mail_history` | `/api/v2/history` | Look up mail history by date range, message IDs, or both. |
| `download_file` | `/api/v2/files/download` | Return bytes, a saved path, or an API error envelope. |
| `create_attachments` or `client.create_attachments` | Local | Read files as binary attachments. |

### Send and batch options

- To, CC and BCC take up to 100 addresses each. To must have a recipient. Deduplication keeps the first match in To → CC → BCC order. Reply-To takes up to 10 addresses and deduplicates independently. One grouped send can include several To addresses.
- The SDK preserves subject, HTML and plain-text contents after validation. Filter context normalizes CRLF and CR to LF and trims PHP whitespace. `DelayUntil` accepts positive Unix seconds through `253402300799` or a `SystemTime` rounded down to whole seconds.
- Batches accept 1–100 messages and expand `cc`, `bcc`, `reply_to`, `attachments` and `filter_context` defaults. An omitted value inherits its default. An empty list or explicit `Nullable::Null` overrides it where the field permits that value.
- Without file parts, the SDK sends JSON. For uploads, `payload` holds the JSON and `attachments[n]` holds the original bytes. Each message accepts up to 100 unique file names. Multipart indices are continuous across the batch. `BinaryAttachment::new` accepts bytes directly.
- An `idempotency_key` is passed unchanged to the server. The API can return a previous send snapshot with `replayed: true` or HTTP 409 with `idempotency_conflict`. To check the current delivery state, query history.

### History and downloads

History accepts `YYYY-MM-DD` strings or UTC dates derived from `SystemTime`. Supply `date_from` when querying without IDs. History and download IDs accept positive `i64` values or decimal strings and are sent as canonical decimal strings. The reference API exposes no pagination parameters.

`DownloadFileOptions::new(DownloadSource::Inbound, "123")` selects memory. Choose `DownloadTarget::Directory(path)` to save under the server's sanitized name or `DownloadTarget::Path(path)` to select a file. An existing directory or trailing separator uses the server name. A download validates the file response before replacing a destination. Use caller-controlled paths for local files and destinations. The SDK holds attachment bytes and responses in memory.

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

### Responses and errors

Valid API envelopes are returned as values at any HTTP status. `http_code` is the actual HTTP status. Typed payloads are available through `data.as_object()`. Unknown fields, new queue statuses, unexpected object schemas, and array envelopes are preserved. Numbers outside JavaScript's safe integer range retain their original JSON spelling as strings. Use `MailStatus` for the known queue statuses. JSON responses are limited to 128 levels of nesting.

`Error` reports input, transport, invalid JSON, invalid envelopes, and local file failures without embedding credentials or response bodies. The client follows no redirects and performs no automatic retries. Dropping the future stops waiting for the HTTP request. A submitted send may still be processed by the server, and an already started file replacement may finish.

## Development

Run `cargo test --locked --all-features` for the local HTTP and filesystem contract suite. Generate public API documentation with `cargo doc --no-deps --open`. GitHub Actions runs formatting, build, Clippy, Rust 1.88 compatibility, dependency review and RustSec checks.

### GitHub automation

- **Branch protection** requires an up-to-date pull request, one current approval, resolved conversations, and the GitHub Actions `Required CI` check. GitHub uses squash merges and deletes merged branches. Repository administrators have a bypass for review and CI. A separate rule blocks force pushes and deletion for everyone. [Review and CI rules](.github/main-ruleset.json) · [History rules](.github/main-history-ruleset.json)
- **Dependabot** checks Cargo and GitHub Actions daily at 03:00 `Europe/Berlin` and groups routine compatible updates separately from security updates. [Dependabot configuration](.github/dependabot.yml)
- **PR maintenance** updates conflict-free branches in this repository for open pull requests targeting `main`. The `reikem-github-pr-maintenance` app can approve and squash-merge signed, compatible Dependabot updates after `Required CI` passes on the current head. Major, potentially breaking pre-1.0 minor, and unclassified updates need manual review. [PR maintenance workflow](.github/workflows/dependabot-automerge.yml)
- **autofix.ci** applies rustfmt and machine-applicable Clippy fixes on pull requests. Its commits rerun CI. [Autofix workflow](.github/workflows/autofix.yml)

Install the official [autofix.ci app](https://github.com/apps/autofix-ci) and the repository-scoped PR maintenance app. Keep the app key in the indicated environment and restrict deployments to `main`. GitHub Actions uses read-only default permissions. Set the environment variable `PR_AUTOMATION_CLIENT_ID` and the encrypted secret `PR_AUTOMATION_PRIVATE_KEY` in `dependabot-maintenance`. To synchronize labels, run `bash .github/scripts/sync-labels.sh`.

## Releases

1. Update `Cargo.toml` and `Cargo.lock` for the next SemVer version. Wait for `Required CI` on `main`.
2. Tag the checked commit as `vMAJOR.MINOR.PATCH`. Run **Rust release** on `main` with that tag to verify the version, CI, tests, and Cargo package. Enable the `publish` input to upload the crate through Trusted Publishing.
3. Create a GitHub Release from that tag. Use the workflow's banner Markdown and the categories from [release.yml](.github/release.yml).

Publish the first version manually with `cargo publish --locked` from the verified tag to register the crate. Then create a Trusted Publisher for `rkstraessler/ReikemMailClient_RS`, workflow `release.yml`, and environment `crates-io`. Restrict that GitHub environment to the `main` branch. [Trusted Publishing](https://crates.io/docs/trusted-publishing) · [Release workflow](.github/workflows/release.yml) · [Release banner](.github/assets/release-banners/reikem-mail-client.png)

## License

[MIT](LICENSE)
