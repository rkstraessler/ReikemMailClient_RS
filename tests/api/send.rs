use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, UNIX_EPOCH};

use reikem_mail_client::{
    ApiPayload, Attachment, BinaryAttachment, DelayUntil, EmailApiClient, EmailAttachment, Error,
    MailAddress, MailStatus, Nullable, ObjectOrArray, QueueStatus,
    SendPersonalizedMailBatchOptions, create_attachments,
};
use serde_json::{Value, json};

use crate::common::{Call, Server, json_response, mail};

#[tokio::test]
async fn single_send_and_alias_share_the_json_contract() {
    let mut api = Server::api().await;
    let client = api.client();
    let mut options = mail();
    options.to = vec![
        address(" USER@Example.COM  ", "  Grüße  ".to_owned().into()),
        address("other@example.com", Nullable::Null),
    ];
    options.cc = Some(vec![
        MailAddress::new("user@example.com"),
        address("CC@example.com", " ".to_owned().into()),
    ]);
    options.bcc = Some(vec![
        MailAddress::new("cc@example.com"),
        MailAddress::new("bcc@example.com"),
    ]);
    options.reply_to = Some(vec![
        MailAddress::new("USER@example.com"),
        MailAddress::new("user@example.com"),
    ]);
    options.subject = "  Betreff  ".into();
    options.html = "\r\n<p>Über uns</p>\r\n".into();
    options.text = Some(String::new());
    options.idempotency_key = Some("\u{feff}Antrag:Version\u{a0}".into());
    options.filter_context = " \tA\r\nB\rC \n".to_owned().into();
    options.delay_until = DelayUntil::Time(UNIX_EPOCH + Duration::from_millis(2_345)).into();

    let first = client.send_mail(&options).await.unwrap();
    let second = client.send_mail_list(&options).await.unwrap();
    assert!(first.is_http_success() && second.is_http_success());
    assert_eq!(first.data.as_object().unwrap().success, Some(true));
    let expected = json!({
        "to": [{"email": "user@example.com", "name": "Grüße"}, {"email": "other@example.com", "name": null}],
        "cc": [{"email": "cc@example.com", "name": null}], "bcc": [{"email": "bcc@example.com"}],
        "replyTo": [{"email": "user@example.com"}], "text": "", "subject": "  Betreff  ",
        "html": "\r\n<p>Über uns</p>\r\n", "idempotencyKey": "\u{feff}Antrag:Version\u{a0}",
        "filterContext": "A\nB\nC", "delayUntil": 2,
    });
    for _ in 0..2 {
        let call = api.next().await;
        call.assert_request("/api/v2/send");
        assert_eq!(call.json(), expected);
    }
    assert_eq!(api.connection_count(), 1);
    assert_eq!(options.to[0].email, " USER@Example.COM  ");
}

#[tokio::test]
async fn multipart_contains_payload_metadata_and_exact_binary_parts() {
    let mut api = Server::api().await;
    let mut options = mail();
    options.attachments = Some(vec![
        BinaryAttachment::new(
            "Grüße \"2026\".pdf",
            "Application/PDF",
            vec![0, 255, 13, 10, 42],
        )
        .into(),
    ]);
    api.client().send_mail(&options).await.unwrap();
    let call = api.next().await;
    call.assert_request("/api/v2/send");
    let parts = multipart_parts(&call);
    assert_eq!(parts.len(), 2);
    assert_eq!(
        parts[0].0,
        "Content-Disposition: form-data; name=\"payload\""
    );
    assert_eq!(
        serde_json::from_slice::<Value>(parts[0].1).unwrap()["attachments"],
        json!([
            {"filename":"Grüße \"2026\".pdf","contentType":"application/pdf","uploadIndex":0}
        ])
    );
    assert!(
        parts[1]
            .0
            .contains("name=\"attachments[0]\"; filename=\"Grüße %222026%22.pdf\"")
    );
    assert!(parts[1].0.ends_with("\r\nContent-Type: application/pdf"));
    assert_eq!(parts[1].1, [0, 255, 13, 10, 42]);
}

#[tokio::test]
async fn batch_expands_defaults_preserves_overrides_and_decodes_partial_results() {
    let mut api = Server::start(|_| json_response(207, r#"{
        "status":207,"data":{"success":false,"queueIds":[45],"sample":9,"results":[
          {"index":0,"success":true,"status":200,"data":{"statuses":[{"status":"future-status",
            "filterDetails":{"ai":{"retry":{"pending":true,"attempt":2,"next_attempt_at":null},
            "result":{"reason_code":"future"}}}}]}},
          {"index":1,"success":false,"status":409,"data":{"success":false,"code":"idempotency_conflict"}}
        ]}}"#)).await;
    let client = api.client();
    let one = mail();
    let mut two = mail();
    two.bcc = Some(vec![]);
    two.reply_to = Some(vec![]);
    two.attachments = Some(vec![]);
    two.filter_context = Nullable::Null;
    two.delay_until = Nullable::Null;
    let mut three = mail();
    three.cc = Some(vec![]);
    let mut batch = SendPersonalizedMailBatchOptions::new(vec![one, two, three]);
    batch.filter_context = " \u{a0}A\u{a0} ".to_owned().into();
    batch.cc = Some(vec![
        MailAddress::new("MAIL@example.com"),
        MailAddress::new("copy@example.com"),
    ]);
    batch.bcc = Some(vec![MailAddress::new("blind@example.com")]);
    batch.reply_to = Some(vec![MailAddress::new("mail@example.com")]);
    batch.attachments = Some(vec![
        BinaryAttachment::new("same.bin", "application/octet-stream", vec![0, 1, 2]).into(),
    ]);

    let response = client.send_personalized_mail_batch(&batch).await.unwrap();
    assert_eq!(response.http_code, 207);
    let data = response.data.as_object().unwrap();
    assert_eq!(data.send.success, Some(false));
    assert_eq!(data.send.extra["sample"], 9);
    let results = data.results.as_ref().unwrap();
    assert_eq!(results[1].status, 409);
    let status = &results[0].data.statuses.as_ref().unwrap()[0];
    assert!(
        matches!(status.status.as_ref(), Some(QueueStatus::Other(value)) if value == "future-status")
    );
    let ObjectOrArray::Object(details) = status.filter_details.as_ref().unwrap() else {
        panic!("AI details")
    };
    let ai = details.ai.as_ref().unwrap();
    assert_eq!(ai.retry.as_ref().unwrap().next_attempt_at, Nullable::Null);
    assert_eq!(
        ai.retry
            .as_ref()
            .unwrap()
            .attempt
            .as_ref()
            .unwrap()
            .as_i64(),
        Some(2)
    );
    assert_eq!(
        ai.result.as_ref().unwrap().reason_code.as_deref(),
        Some("future")
    );
    assert_eq!(MailStatus::all().len(), 8);
    assert!(MailStatus::is_known("queued"));
    assert!(!MailStatus::is_known("future-status"));

    let call = api.next().await;
    call.assert_request("/api/v2/send-batch");
    let parts = multipart_parts(&call);
    assert_eq!(parts.len(), 3);
    let payload: Value = serde_json::from_slice(parts[0].1).unwrap();
    assert_eq!(payload.as_object().unwrap().len(), 1);
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages[0]["attachments"][0]["uploadIndex"], 0);
    assert_eq!(messages[0]["filterContext"], "\u{a0}A\u{a0}");
    assert_eq!(messages[0]["cc"], json!([{"email":"copy@example.com"}]));
    assert_eq!(
        messages[0]["replyTo"],
        json!([{"email":"mail@example.com"}])
    );
    for key in ["attachments", "bcc", "replyTo"] {
        assert_eq!(messages[1][key], json!([]));
    }
    assert_eq!(messages[1]["filterContext"], Value::Null);
    assert!(messages[1].get("delayUntil").is_some_and(Value::is_null));
    assert!(messages[0].get("delayUntil").is_none());
    assert_eq!(messages[2]["attachments"][0]["uploadIndex"], 1);
    assert_eq!(messages[2]["cc"], json!([]));
    for (index, part) in parts[1..].iter().enumerate() {
        assert!(part.0.contains(&format!("name=\"attachments[{index}]\"")));
        assert_eq!(part.1, [0, 1, 2]);
    }
    assert!(batch.messages[0].attachments.is_none());
}

#[tokio::test]
async fn sends_attachment_free_batches_as_json_at_the_batch_limit() {
    let mut api = Server::api().await;
    let mut batch = SendPersonalizedMailBatchOptions::new(vec![mail(); 100]);
    batch.attachments = Some(vec![]);
    batch.filter_context = " \t \n ".to_owned().into();
    api.client()
        .send_personalized_mail_batch(&batch)
        .await
        .unwrap();
    let call = api.next().await;
    let payload = call.json();
    assert_eq!(payload.as_object().unwrap().len(), 1);
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 100);
    for message in messages {
        assert_eq!(message["attachments"], json!([]));
        assert!(message.get("filterContext").is_some_and(Value::is_null));
        assert!(message.get("idempotencyKey").is_none());
    }
}

#[tokio::test]
async fn loads_local_files_and_accepts_the_legacy_adapter() {
    let mut api = Server::api().await;
    let client = api.client();
    let directory = tempfile::tempdir().unwrap();
    let pdf = directory.path().join("Vertrag.PDF");
    let xml = directory.path().join("Data.xml");
    let unknown = directory.path().join("File.unknown-rk-extension");
    for path in [&pdf, &xml, &unknown] {
        tokio::fs::write(path, b"file bytes").await.unwrap();
    }

    let files = create_attachments(&[&pdf, &xml, &unknown]).await.unwrap();
    assert_eq!(files[0].content_type, "application/pdf");
    assert_eq!(files[1].content_type, "application/xml");
    assert_eq!(files[2].content_type, "application/octet-stream");
    let legacy = EmailAttachment::new("Legacy.pdf", "application/pdf", &pdf, 2);
    let mut options = mail();
    let mut attachments = client
        .create_attachments(&[&pdf])
        .await
        .unwrap()
        .into_iter()
        .map(Attachment::from)
        .collect::<Vec<_>>();
    attachments.push(legacy.into());
    options.attachments = Some(attachments);
    client.send_mail(&options).await.unwrap();
    let call = api.next().await;
    let parts = multipart_parts(&call);
    assert_eq!(parts[1].1, b"file bytes");
    assert_eq!(parts[2].1, b"file bytes");
    assert!(parts[2].0.contains("Legacy.pdf"));
    assert!(matches!(
        create_attachments(&[directory.path().join("missing.bin")]).await,
        Err(Error::AttachmentRead(_))
    ));
    assert!(matches!(
        create_attachments(&[&pdf, &pdf]).await,
        Err(Error::InvalidInput { .. })
    ));
    assert!(matches!(
        create_attachments(&[directory.path()]).await,
        Err(Error::AttachmentRead(_))
    ));
    assert!(matches!(
        create_attachments(&[&pdf; 101]).await,
        Err(Error::InvalidInput { .. })
    ));
}

#[tokio::test]
async fn validates_send_limits_before_http() {
    let mut api = Server::api().await;
    let client = api.client();
    let check = |mutate: fn(&mut reikem_mail_client::SendMailOptions)| {
        let mut options = mail();
        mutate(&mut options);
        options
    };
    let invalid = [
        ("to", check(|options| options.to.clear())),
        (
            "to",
            check(|options| options.to = vec![MailAddress::new("a..b@example.com")]),
        ),
        (
            "to",
            check(|options| options.to = vec![MailAddress::new("\tuser@example.com")]),
        ),
        (
            "to",
            check(|options| options.to = vec![MailAddress::new("\u{feff}user@example.com")]),
        ),
        (
            "to",
            check(|options| options.to[0].name = "bad\nname".to_owned().into()),
        ),
        (
            "cc",
            check(|options| options.cc = Some(vec![MailAddress::new("same@example.com"); 101])),
        ),
        (
            "bcc",
            check(|options| options.bcc = Some(vec![MailAddress::new("same@example.com"); 101])),
        ),
        (
            "replyTo",
            check(|options| {
                options.reply_to = Some(vec![MailAddress::new("same@example.com"); 11])
            }),
        ),
        (
            "subject",
            check(|options| options.subject = "😀".repeat(5_001)),
        ),
        (
            "subject",
            check(|options| options.subject = " \u{feff}".into()),
        ),
        ("html", check(|options| options.html = " \r\n".into())),
        (
            "idempotencyKey",
            check(|options| options.idempotency_key = Some("👋".repeat(256))),
        ),
        (
            "idempotencyKey",
            check(|options| options.idempotency_key = Some(" key".into())),
        ),
        (
            "idempotencyKey",
            check(|options| options.idempotency_key = Some("key\u{85}".into())),
        ),
        (
            "filterContext",
            check(|options| options.filter_context = "😀".repeat(1_001).into()),
        ),
        (
            "filterContext",
            check(|options| options.filter_context = "\0context".to_owned().into()),
        ),
        (
            "delayUntil",
            check(|options| options.delay_until = DelayUntil::Seconds(253_402_300_800).into()),
        ),
        (
            "delayUntil",
            check(|options| options.delay_until = DelayUntil::Text("1.5".into()).into()),
        ),
        (
            "delayUntil",
            check(|options| options.delay_until = DelayUntil::Seconds(0).into()),
        ),
        (
            "attachment.filename",
            check(|options| {
                options.attachments = Some(vec![
                    BinaryAttachment::new("../bad.pdf", "application/pdf", vec![]).into(),
                ])
            }),
        ),
        (
            "attachment.filename",
            check(|options| {
                options.attachments = Some(vec![
                    BinaryAttachment::new("😀".repeat(501), "application/pdf", vec![]).into(),
                ])
            }),
        ),
        (
            "attachment.contentType",
            check(|options| {
                options.attachments = Some(vec![
                    BinaryAttachment::new("a.txt", "text/plain; charset=utf-8", vec![]).into(),
                ])
            }),
        ),
        (
            "attachments",
            check(|options| {
                options.attachments = Some(vec![
                    BinaryAttachment::new("again", "text/plain", vec![])
                        .into();
                    2
                ])
            }),
        ),
    ];
    for (field, options) in invalid {
        assert!(
            matches!(client.send_mail(&options).await, Err(Error::InvalidInput { field: actual, .. }) if actual == field),
            "{field}"
        );
    }
    assert_eq!(EmailApiClient::MAX_FILTER_CONTEXT_CHARACTERS, 1_000);
    let mut batch = SendPersonalizedMailBatchOptions::new(vec![mail(); 101]);
    assert!(matches!(
        client.send_personalized_mail_batch(&batch).await,
        Err(Error::InvalidInput {
            field: "messages",
            ..
        })
    ));
    batch.messages = vec![mail()];
    batch.messages[0].filter_context = Nullable::Null;
    batch.filter_context = "😀".repeat(1_001).into();
    assert!(matches!(
        client.send_personalized_mail_batch(&batch).await,
        Err(Error::InvalidInput {
            field: "filterContext",
            ..
        })
    ));
    api.assert_empty();
}

#[tokio::test]
async fn validates_attachment_count_per_message_and_accepts_inclusive_limits() {
    let mut api = Server::api().await;
    let client = api.client();
    let mut options = mail();
    options.subject = "😀".repeat(5_000);
    options.idempotency_key = Some("😀".repeat(255));
    options.filter_context = "😀".repeat(1_000).into();
    options.delay_until = DelayUntil::Text(" 000253402300799 ".into()).into();
    options.attachments = Some(
        (0..100)
            .map(|index| {
                BinaryAttachment::new(format!("{index}.txt"), "text/plain", bytes::Bytes::new())
                    .into()
            })
            .collect(),
    );
    let mut batch = SendPersonalizedMailBatchOptions::new(vec![options.clone(), options.clone()]);
    client.send_personalized_mail_batch(&batch).await.unwrap();
    let call = api.next().await;
    let parts = multipart_parts(&call);
    assert_eq!(parts.len(), 201);
    let payload: Value = serde_json::from_slice(parts[0].1).unwrap();
    assert_eq!(
        payload["messages"][1]["attachments"][99]["uploadIndex"],
        199
    );
    assert_eq!(payload["messages"][0]["delayUntil"], 253_402_300_799_i64);

    let upload = BinaryAttachment::new("extra.txt", "text/plain", bytes::Bytes::new()).into();
    options.attachments.as_mut().unwrap().push(upload);
    assert!(matches!(
        client.send_mail(&options).await,
        Err(Error::InvalidInput {
            field: "attachments",
            ..
        })
    ));
    batch.attachments = options.attachments.clone();
    assert!(matches!(
        client.send_personalized_mail_batch(&batch).await,
        Err(Error::InvalidInput {
            field: "attachments",
            ..
        })
    ));
    batch.attachments = None;
    batch.messages[0] = options;
    assert!(matches!(
        client.send_personalized_mail_batch(&batch).await,
        Err(Error::InvalidInput {
            field: "attachments",
            ..
        })
    ));
    api.assert_empty();
}

#[tokio::test]
async fn accepts_quoted_mailboxes_and_ip_literals() {
    let mut api = Server::api().await;
    let mut options = mail();
    options.to = vec![
        MailAddress::new(r#""A@B\"C"@[IPv6:2001:DB8::1]"#),
        MailAddress::new("Admin@[192.0.2.1]"),
    ];
    api.client().send_mail(&options).await.unwrap();
    assert_eq!(
        api.next().await.json()["to"],
        json!([
            {"email":r#""a@b\"c"@[ipv6:2001:db8::1]"#}, {"email":"admin@[192.0.2.1]"}
        ])
    );
}

#[tokio::test]
async fn returns_idempotency_replays_and_conflicts_without_local_retries() {
    let calls = AtomicUsize::new(0);
    let mut api = Server::start(move |_| match calls.fetch_add(1, Ordering::Relaxed) {
        0 => json_response(200, r#"{"status":200,"data":{"queueIds":[45],"replayed":false}}"#),
        1 => json_response(200, r#"{"status":200,"data":{"queueIds":[45],"replayed":true}}"#),
        _ => json_response(409, r#"{"status":409,"data":{"success":false,"error":"Konflikt","code":"idempotency_conflict"}}"#),
    }).await;
    let client = api.client();
    let mut options = mail();
    options.idempotency_key = Some("Business:K".into());
    assert_eq!(
        client
            .send_mail(&options)
            .await
            .unwrap()
            .data
            .as_object()
            .unwrap()
            .replayed,
        Some(false)
    );
    assert_eq!(
        client
            .send_mail(&options)
            .await
            .unwrap()
            .data
            .as_object()
            .unwrap()
            .replayed,
        Some(true)
    );
    options.html = "different HTML".into();
    let conflict = client.send_mail(&options).await.unwrap();
    assert!(conflict.is_idempotency_conflict());
    let ApiPayload::Object(data) = conflict.data else {
        panic!("typed conflict")
    };
    assert_eq!(data.error, Nullable::Value(json!("Konflikt")));
    for _ in 0..3 {
        assert_eq!(api.next().await.json()["idempotencyKey"], "Business:K");
    }
    api.assert_empty();
}

fn address(email: &str, name: Nullable<String>) -> MailAddress {
    MailAddress {
        email: email.into(),
        name,
    }
}

fn multipart_parts(call: &Call) -> Vec<(&str, &[u8])> {
    let boundary = call.headers["content-type"]
        .to_str()
        .unwrap()
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap();
    let mut rest = call
        .body
        .strip_prefix(format!("--{boundary}\r\n").as_bytes())
        .unwrap();
    let marker = format!("\r\n--{boundary}");
    let mut parts = Vec::new();
    while rest != b"--\r\n" {
        let (header, body) = split_at_marker(rest, b"\r\n\r\n");
        let (content, next) = split_at_marker(body, marker.as_bytes());
        parts.push((std::str::from_utf8(header).unwrap(), content));
        rest = next.strip_prefix(b"\r\n").unwrap_or(next);
    }
    parts
}

fn split_at_marker<'a>(bytes: &'a [u8], marker: &[u8]) -> (&'a [u8], &'a [u8]) {
    let index = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .unwrap();
    (&bytes[..index], &bytes[index + marker.len()..])
}
