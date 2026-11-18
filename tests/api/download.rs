use std::path::PathBuf;

use bytes::Bytes;
use http_body_util::Full;
use hyper::Response;
use hyper::header::HeaderValue;
use reikem_mail_client::{
    ApiInteger, DownloadFileOptions, DownloadOutcome, DownloadSource, DownloadTarget,
    DownloadedFile, Error,
};
use serde_json::json;

use crate::common::{MockResponse, Server, json_response};

#[tokio::test]
async fn downloads_binary_bytes_with_the_normalized_v2_request() {
    let mut api = Server::start(|_| {
        file_response(
            b"\0\xff\x80\n\r*",
            Some("application/octet-stream"),
            Some(b"attachment; filename*=UTF-8''Gr%C3%BC%C3%9Fe.pdf; filename=ignored.pdf"),
        )
    })
    .await;
    let options = DownloadFileOptions::new(" OUTBOUND ".parse().unwrap(), "9223372036854775807");
    let result = api.client().download_file(&options).await.unwrap();
    let call = api.next().await;
    call.assert_request("/api/v2/files/download");
    assert_eq!(
        call.json(),
        json!({"source":"outbound","fileId":"9223372036854775807"})
    );
    let DownloadOutcome::File(file) = result else {
        panic!("file")
    };
    assert_eq!(file.file_name, "Grüße.pdf");
    assert_eq!(file.bytes, 6);
    assert_eq!(file.http_code, 200);
    assert_eq!(
        file.content_type.as_deref(),
        Some("application/octet-stream")
    );
    assert!(
        matches!(file.file, DownloadedFile::Content(bytes) if bytes.as_ref() == b"\0\xff\x80\n\r*")
    );
}

#[tokio::test]
async fn supports_utf8_and_latin1_filenames_and_removes_untrusted_path_parts() {
    let cases: &[(&[u8], &str)] = &[
        ("attachment; filename=\"Grüße.pdf\"".as_bytes(), "Grüße.pdf"),
        (b"attachment; filename=\"caf\xe9.pdf\"", "café.pdf"),
        (b"attachment; filename=\"..\\..\\safe.pdf\"", "safe.pdf"),
        (
            b"attachment; filename=\"name;with;semicolons.txt\"",
            "name;with;semicolons.txt",
        ),
        (b"attachment; Filename*=utf-8''..%2Fsafe.pdf", "safe.pdf"),
        (
            b"attachment; filename*=UTF-8''%ZZ;filename=ignored.pdf",
            "download",
        ),
        (b"attachment; filename*=UTF-8''%FF", "download"),
        (b"attachment; filename*=UTF-8''C%3Aevil.pdf", "download"),
        (b"attachment; filename=\"..\"", "download"),
        (b"attachment", "download"),
    ];
    for &(header, expected) in cases {
        let api = Server::start(move |_| file_response(b"bytes", None, Some(header))).await;
        let options = DownloadFileOptions::new(DownloadSource::Inbound, 1);
        let DownloadOutcome::File(file) = api.client().download_file(&options).await.unwrap()
        else {
            panic!("file")
        };
        assert_eq!(file.file_name, expected);
        assert!(file.content_type.is_none());
    }
}

#[tokio::test]
async fn writes_exact_paths_and_directory_targets_without_partial_files() {
    let api = Server::start(|_| {
        file_response(
            b"new bytes",
            None,
            Some(b"attachment; filename=\"doc.txt\""),
        )
    })
    .await;
    let root = tempfile::tempdir().unwrap();
    let client = api.client();
    let exact = root.path().join("missing-parent").join("local.bin");
    let slash = root.path().join("auto-dir").join("");
    let cases = [
        (DownloadTarget::Path(exact.clone()), exact.clone()),
        (
            DownloadTarget::Path(root.path().to_owned()),
            root.path().join("doc.txt"),
        ),
        (
            DownloadTarget::Directory(root.path().join("new-dir")),
            root.path().join("new-dir").join("doc.txt"),
        ),
        (
            DownloadTarget::Path(slash),
            root.path().join("auto-dir").join("doc.txt"),
        ),
    ];
    for (target, expected) in cases {
        let mut options = DownloadFileOptions::new(DownloadSource::Inbound, 1);
        options.target = target;
        let DownloadOutcome::File(file) = client.download_file(&options).await.unwrap() else {
            panic!("file")
        };
        assert_eq!(file.file, DownloadedFile::Saved(expected.clone()));
        assert_eq!(tokio::fs::read(&expected).await.unwrap(), b"new bytes");
        assert!(
            std::fs::read_dir(expected.parent().unwrap())
                .unwrap()
                .all(|item| !item
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".reikem-mail-"))
        );
    }
    tokio::fs::write(&exact, b"old bytes").await.unwrap();
    let mut options = DownloadFileOptions::new(DownloadSource::Inbound, 1);
    options.target = DownloadTarget::Path(exact.clone());
    client.download_file(&options).await.unwrap();
    assert_eq!(tokio::fs::read(&exact).await.unwrap(), b"new bytes");
    options.target = DownloadTarget::Directory(exact);
    assert!(matches!(
        client.download_file(&options).await,
        Err(Error::DownloadWrite(_))
    ));
}

#[tokio::test]
async fn returns_api_errors_and_refuses_mislabeled_envelopes_before_touching_disk() {
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("keep.pdf");
    tokio::fs::write(&target, b"keep me").await.unwrap();
    let mut options = DownloadFileOptions::new(DownloadSource::Inbound, 1);
    options.target = DownloadTarget::Path(target.clone());
    let api = Server::start(|_| {
        json_response(
            404,
            r#"{"status":404,"data":{"queueIds":[9223372036854775807]}}"#,
        )
    })
    .await;
    let DownloadOutcome::ApiError(result) = api.client().download_file(&options).await.unwrap()
    else {
        panic!("API error")
    };
    assert_eq!(result.http_code, 404);
    assert_eq!(
        result
            .data
            .as_object()
            .unwrap()
            .send
            .queue_ids
            .as_ref()
            .unwrap(),
        &[ApiInteger::String(i64::MAX.to_string())]
    );
    for content_type in [
        "application/json",
        "APPLICATION/PROBLEM+JSON; charset=utf-8",
        "application/octet-stream",
    ] {
        let api = Server::start(move |_| {
            file_response(
                b" \xef\xbb\xbf {\"status\":422,\"data\":[]}",
                Some(content_type),
                None,
            )
        })
        .await;
        assert!(matches!(
            api.client().download_file(&options).await,
            Err(Error::JsonInsteadOfFile { http_code: 200 })
        ));
    }
    let api = Server::start(|_| json_response(500, "PRIVATE_DOWNLOAD_RESPONSE")).await;
    assert!(matches!(
        api.client().download_file(&options).await,
        Err(Error::InvalidJson { http_code: 500 })
    ));
    assert_eq!(tokio::fs::read(&target).await.unwrap(), b"keep me");
}

#[tokio::test]
async fn permits_files_with_json_like_contents_and_rejects_invalid_options() {
    let mut api =
        Server::start(|_| file_response(b"{\"notAnEnvelope\":[]}", Some("text/plain"), None)).await;
    let client = api.client();
    let options = DownloadFileOptions::new(DownloadSource::Outbound, 1);
    assert!(matches!(
        client.download_file(&options).await.unwrap(),
        DownloadOutcome::File(_)
    ));
    api.next().await;
    assert!("other".parse::<DownloadSource>().is_err());
    for id in ["0", "-42", "12junk", "9223372036854775808"] {
        let options = DownloadFileOptions::new(DownloadSource::Inbound, id);
        assert!(matches!(
            client.download_file(&options).await,
            Err(Error::InvalidInput {
                field: "fileId",
                ..
            })
        ));
    }
    for path in ["", "a\0b"] {
        let mut options = DownloadFileOptions::new(DownloadSource::Inbound, 1);
        options.target = DownloadTarget::Path(PathBuf::from(path));
        assert!(matches!(
            client.download_file(&options).await,
            Err(Error::InvalidInput {
                field: "targetPath",
                ..
            })
        ));
    }
    api.assert_empty();
}

fn file_response(
    body: &'static [u8],
    media_type: Option<&str>,
    name: Option<&[u8]>,
) -> MockResponse {
    let mut response = Response::builder().status(200);
    if let Some(media_type) = media_type {
        response = response.header("content-type", media_type);
    }
    if let Some(name) = name {
        response = response.header(
            "content-disposition",
            HeaderValue::from_bytes(name).unwrap(),
        );
    }
    response.body(Full::new(Bytes::from_static(body))).unwrap()
}
