use std::convert::Infallible;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::header::HeaderMap;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response};
use hyper_util::rt::TokioIo;
use reikem_mail_client::{EmailApiClient, MailAddress, SendMailOptions};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;

pub const AUTH_KEY: &str = "test-auth";
pub const ADAPTER_KEY: &str = "test-adapter";

pub struct Call {
    pub method: Method,
    pub path: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Call {
    pub fn assert_request(&self, path: &str) {
        assert_eq!(self.method, Method::POST);
        assert_eq!(self.path, path);
        assert_eq!(self.headers["authorization"], "Bearer test-auth");
        assert_eq!(self.headers["x-adapter-key"], ADAPTER_KEY);
        assert_eq!(self.headers["content-length"], self.body.len().to_string());
        assert!(!self.headers.contains_key("accept-encoding"));
    }

    pub fn json(&self) -> Value {
        assert_eq!(self.headers["content-type"], "application/json");
        serde_json::from_slice(&self.body).unwrap()
    }
}

pub type MockResponse = Response<Full<Bytes>>;

pub struct Server {
    pub url: String,
    calls: UnboundedReceiver<Call>,
    task: JoinHandle<()>,
    connections: Arc<AtomicUsize>,
}

impl Server {
    pub async fn api() -> Self {
        Self::start(|_| json_response(200, r#"{"status":200,"data":{"success":true}}"#)).await
    }

    pub async fn start(respond: impl Fn(&Call) -> MockResponse + Send + Sync + 'static) -> Self {
        let tcp = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let url = format!("http://{}", tcp.local_addr().unwrap());
        let (calls_tx, calls) = mpsc::unbounded_channel();
        let connections = Arc::new(AtomicUsize::new(0));
        let connection_count = connections.clone();
        let respond = Arc::new(respond);
        let task = tokio::spawn(async move {
            let mut handlers = JoinSet::new();
            loop {
                tokio::select! {
                    socket = tcp.accept() => {
                        let (socket, _) = socket.unwrap();
                        connection_count.fetch_add(1, Ordering::Relaxed);
                        let respond = respond.clone();
                        let calls = calls_tx.clone();
                        handlers.spawn(async move {
                            let service = service_fn(|request: Request<Incoming>| {
                                let calls = calls.clone();
                                let respond = respond.clone();
                                async move {
                                    let (parts, body) = request.into_parts();
                                    let call = Call {
                                        method: parts.method,
                                        path: parts.uri.to_string(),
                                        headers: parts.headers,
                                        body: body.collect().await.unwrap().to_bytes(),
                                    };
                                    let response = respond(&call);
                                    let _ = calls.send(call);
                                    Ok::<_, Infallible>(response)
                                }
                            });
                            let _ = http1::Builder::new().serve_connection(TokioIo::new(socket), service).await;
                        });
                    }
                    _ = handlers.join_next(), if !handlers.is_empty() => {}
                }
            }
        });

        Self {
            url,
            calls,
            task,
            connections,
        }
    }

    pub fn client(&self) -> EmailApiClient {
        EmailApiClient::from_credentials(&self.url, AUTH_KEY, ADAPTER_KEY).unwrap()
    }

    pub async fn next(&mut self) -> Call {
        timeout(Duration::from_secs(4), self.calls.recv())
            .await
            .unwrap()
            .expect("one API call")
    }

    pub fn assert_empty(&mut self) {
        assert!(matches!(
            self.calls.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    pub fn connection_count(&self) -> usize {
        self.connections.load(Ordering::Relaxed)
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn mail() -> SendMailOptions {
    SendMailOptions::new(vec![MailAddress::new("mail@example.com")], "S", "<p>H</p>")
}

pub fn json_response(http_code: u16, body: impl Into<Bytes>) -> MockResponse {
    Response::builder()
        .status(http_code)
        .header("content-type", "application/json")
        .body(Full::new(body.into()))
        .unwrap()
}
