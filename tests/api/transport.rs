use std::io::ErrorKind;
use std::time::Duration;

use bytes::Bytes;
use reikem_mail_client::{ClientConfig, EmailApiClient, Error, TransportErrorKind};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::time::timeout;

use crate::common::{ADAPTER_KEY, AUTH_KEY, Server, json_response, mail};

#[test]
fn rejects_invalid_configuration_without_leaking_credentials() {
    for url in [
        "not a URL",
        "ftp://example.com",
        "https://user:secret@example.com",
        "https://example.com/?key=secret",
        "https://example.com/#fragment",
    ] {
        let error = ClientConfig::new(url, AUTH_KEY, ADAPTER_KEY).unwrap_err();
        assert!(matches!(error, Error::InvalidBaseUrl));
        assert!(!format!("{error:?} {error}").contains("secret"));
    }
    for credential in [
        "",
        "leading ",
        "internal space",
        "key\t",
        "a\r\nb",
        "key\x7f",
        "clé",
    ] {
        assert!(matches!(
            ClientConfig::new("https://example.com", credential, ADAPTER_KEY),
            Err(Error::InvalidCredential {
                header: "Authorization"
            })
        ));
        assert!(matches!(
            ClientConfig::new("https://example.com", AUTH_KEY, credential),
            Err(Error::InvalidCredential {
                header: "X-Adapter-Key"
            })
        ));
    }
    let origin =
        || ClientConfig::new("https://example.com/known-path", AUTH_KEY, ADAPTER_KEY).unwrap();
    for invalid in [
        Duration::ZERO,
        Duration::from_millis(1_500),
        Duration::from_secs(2_147_484),
    ] {
        assert!(matches!(
            origin().with_timeouts(invalid, Duration::from_secs(30)),
            Err(Error::InvalidTimeout)
        ));
        assert!(matches!(
            origin().with_timeouts(Duration::from_secs(10), invalid),
            Err(Error::InvalidTimeout)
        ));
    }
    assert!(
        origin()
            .with_timeouts(Duration::from_secs(1), Duration::from_secs(2_147_483))
            .is_ok()
    );
    let config_debug = format!("{:?}", origin());
    let client_debug = format!("{:?}", EmailApiClient::new(origin()).unwrap());
    for debug in [config_debug, client_debug] {
        assert!(
            !debug.contains(AUTH_KEY)
                && !debug.contains(ADAPTER_KEY)
                && !debug.contains("known-path")
        );
    }
    fn send_sync<T: Send + Sync>() {}
    send_sync::<EmailApiClient>();
}

#[tokio::test]
async fn builds_the_same_v2_path_from_every_supported_base_url_shape() {
    let mut api = Server::api().await;
    for (suffix, relative) in [
        ("", ""),
        ("/", ""),
        ("/proxy", "/proxy"),
        ("/proxy///", "/proxy"),
        ("/proxy/api/send", "/proxy"),
        ("/proxy/api/send/", "/proxy"),
        ("/proxy/api/v2/send", "/proxy"),
        ("/proxy/api/v2/", "/proxy"),
        ("/proxy%20one/api/v2", "/proxy%20one"),
    ] {
        let url = format!("{}{suffix}", api.url);
        let client = EmailApiClient::from_credentials(&url, AUTH_KEY, ADAPTER_KEY).unwrap();
        client.send_mail(&mail()).await.unwrap();
        api.next()
            .await
            .assert_request(&format!("{relative}/api/v2/send"));
    }
}

#[tokio::test]
async fn sends_printable_non_space_credential_boundaries_without_rewriting() {
    let mut api = Server::api().await;
    let client = EmailApiClient::from_credentials(&api.url, "!auth~", "~adapter!").unwrap();
    client.send_mail(&mail()).await.unwrap();
    let call = api.next().await;
    assert_eq!(call.headers["authorization"], "Bearer !auth~");
    assert_eq!(call.headers["x-adapter-key"], "~adapter!");
}

#[tokio::test]
async fn returns_http_errors_without_redirects_or_retries() {
    let mut api = Server::start(|_| {
        let mut response = json_response(302, r#"{"status":302,"data":{"success":false}}"#);
        response
            .headers_mut()
            .insert("location", "/redirected".parse().unwrap());
        response
    })
    .await;
    let response = api.client().send_mail(&mail()).await.unwrap();
    assert_eq!(response.http_code, 302);
    assert!(!response.is_http_success());
    assert_eq!(api.next().await.path, "/api/v2/send");
    api.assert_empty();

    let mut api = Server::start(|_| json_response(503, r#"{"status":503,"data":{}}"#)).await;
    assert_eq!(
        api.client().send_mail(&mail()).await.unwrap().http_code,
        503
    );
    api.next().await;
    api.assert_empty();
}

#[tokio::test]
async fn rejects_invalid_json_and_envelopes_without_exposing_the_response() {
    let cases = [
        (&b"PRIVATE_RESPONSE"[..], true),
        (&b"[]"[..], false),
        (&b"\"PRIVATE_RESPONSE\""[..], false),
        (&b"{\"status\":200}"[..], false),
        (&b"{\"status\":200,\"data\":null}"[..], false),
        (&b"{\"status\":\"200\",\"data\":{}}"[..], false),
        (&b"{\"status\":1.5,\"data\":{}}"[..], false),
        (&b"{\"status\":9007199254740999,\"data\":{}}"[..], false),
        (&b"\xff"[..], true),
        (&b"{\"status\":200,\"data\":{\"x\":01}}"[..], true),
        (&b"{\"status\":200,\"data\":{\"x\":\"\xff\"}}"[..], true),
    ];
    for (body, invalid_json) in cases {
        let api = Server::start(move |_| json_response(502, Bytes::from_static(body))).await;
        let error = api.client().send_mail(&mail()).await.unwrap_err();
        if invalid_json {
            assert!(
                matches!(error, Error::InvalidJson { http_code: 502 }),
                "{body:?}: {error}"
            );
        } else {
            assert!(
                matches!(error, Error::InvalidEnvelope { http_code: 502 }),
                "{body:?}: {error}"
            );
        }
        assert!(!format!("{error:?} {error}").contains("PRIVATE_RESPONSE"));
    }
}

#[tokio::test]
async fn reports_a_connection_failure_when_no_server_is_listening() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    drop(listener);
    let client = EmailApiClient::from_credentials(&url, AUTH_KEY, ADAPTER_KEY).unwrap();
    assert!(matches!(
        client.send_mail(&mail()).await,
        Err(Error::Transport(TransportErrorKind::Connection))
    ));
}

#[tokio::test]
async fn reports_disconnected_bodies_without_replaying_a_send() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let client = EmailApiClient::from_credentials(&url, AUTH_KEY, ADAPTER_KEY).unwrap();
    let handler = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        send_partial_body(&mut socket).await;
        socket.shutdown().await.unwrap();
    };
    let message = mail();
    let (result, ()) = timeout(Duration::from_secs(4), async {
        tokio::join!(client.send_mail(&message), handler)
    })
    .await
    .unwrap();
    assert!(matches!(result, Err(Error::Transport(_))));
    assert!(
        timeout(Duration::from_millis(100), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn applies_the_total_timeout_to_a_stalled_response_body() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let client = test_client(&listener, "http", 4, 1);
    let message = mail();
    let operation = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        send_partial_body(&mut socket).await;
        let mut bytes = Vec::new();
        read_until_closed(&mut socket, &mut bytes).await;
    };
    let (result, ()) = timeout(Duration::from_secs(4), async {
        tokio::join!(client.send_mail(&message), operation)
    })
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(Error::Transport(TransportErrorKind::Timeout))
    ));
}

#[tokio::test]
async fn applies_the_connect_timeout_to_an_incomplete_tls_handshake() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let client = test_client(&listener, "https", 1, 8);
    let message = mail();
    let operation = async {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut client_hello = Vec::new();
        read_until_closed(&mut socket, &mut client_hello).await;
        assert_eq!(client_hello.first(), Some(&0x16));
    };
    let (result, ()) = timeout(Duration::from_secs(4), async {
        tokio::join!(client.send_mail(&message), operation)
    })
    .await
    .unwrap();
    assert!(matches!(
        result,
        Err(Error::Transport(TransportErrorKind::Timeout))
    ));
}

#[tokio::test]
async fn cancels_waiting_for_an_in_flight_response_when_the_future_is_dropped() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let client = test_client(&listener, "http", 10, 15);
    let (started, ready) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        send_partial_body(&mut socket).await;
        started.send(()).unwrap();
        let mut rest = Vec::new();
        read_until_closed(&mut socket, &mut rest).await;
    });
    let request = tokio::spawn(async move { client.send_mail(&mail()).await });
    timeout(Duration::from_secs(4), ready)
        .await
        .unwrap()
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    timeout(Duration::from_secs(4), server)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reuses_the_client_concurrently() {
    let mut api = Server::api().await;
    let client = api.client();
    let mut requests = tokio::task::JoinSet::new();
    for index in 0..8 {
        let client = client.clone();
        requests.spawn(async move {
            let mut options = mail();
            options.idempotency_key = Some(index.to_string());
            client.send_mail(&options).await.unwrap().http_code
        });
    }
    while let Some(result) = requests.join_next().await {
        assert_eq!(result.unwrap(), 200);
    }
    let mut received = Vec::new();
    for _ in 0..8 {
        let call = api.next().await;
        call.assert_request("/api/v2/send");
        received.push(call.json()["idempotencyKey"].as_str().unwrap().to_owned());
    }
    received.sort();
    assert_eq!(received, ["0", "1", "2", "3", "4", "5", "6", "7"]);
}

fn test_client(listener: &TcpListener, scheme: &str, connect: u64, total: u64) -> EmailApiClient {
    let url = format!("{scheme}://{}", listener.local_addr().unwrap());
    let config = ClientConfig::new(&url, AUTH_KEY, ADAPTER_KEY)
        .unwrap()
        .with_timeouts(Duration::from_secs(connect), Duration::from_secs(total))
        .unwrap();
    EmailApiClient::new(config).unwrap()
}

async fn send_partial_body(socket: &mut TcpStream) {
    let mut bytes = [0; 4096];
    assert_ne!(socket.read(&mut bytes).await.unwrap(), 0);
    socket
        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{")
        .await
        .unwrap();
}

async fn read_until_closed(socket: &mut TcpStream, bytes: &mut Vec<u8>) {
    if let Err(error) = socket.read_to_end(bytes).await {
        assert_eq!(error.kind(), ErrorKind::ConnectionReset);
    }
}
