//! Local HTTP and filesystem contract tests for the public SDK.

mod common;

#[path = "api/download.rs"]
mod download;
#[path = "api/history.rs"]
mod history;
#[path = "api/send.rs"]
mod send;
#[path = "api/transport.rs"]
mod transport;
