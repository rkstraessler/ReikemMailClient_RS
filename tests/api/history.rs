use std::time::{Duration, UNIX_EPOCH};

use reikem_mail_client::{
    ApiInteger, ApiPayload, Error, GetMailHistoryOptions, HistoryMessageIds, MailStatus, Nullable,
    ObjectOrArray, QueueStatus,
};
use serde_json::{Value, json};

use crate::common::{Server, json_response};

#[tokio::test]
async fn sends_canonical_history_filters_and_preserves_large_response_ids() {
    let mut api = Server::start(|_| json_response(200, r#"{
        "status":2e2,"httpCode":999,"marker":"future","data":{
            "queueIds":[42,9007199254740993],"future":1e400,"overflown":-1E+400,
            "history":{"Outbound":[{"queueId":9223372036854775807,"messageId":"123","subject":"S",
              "html":"H","to":[{"email":"a@example.com"}],"cc":[],"bcc":[],"replyTo":[],
              "idempotencyKey":null,"status":"review","filterDetails":[],"notes":"outbound"}],
              "Inbound":[{"regard":"Subject","content":"Body","replyTo":[{"email":"b@example.com","name":null}]}],
              "unknownDirection":[]},
            "9007199254740993":"literal 9223372036854775807", "nested":[9E30,1e+30,-9007199254740993],
            "escaped":"\"9007199254740993\"","safe":42, "filters":{"rejections":[], "reasons":["r"]}
        }}"#)).await;
    let filters = GetMailHistoryOptions {
        date_from: Some((UNIX_EPOCH + Duration::from_secs(1_709_216_200)).into()),
        date_to: Some(" 2024-03-01 ".into()),
        message_ids: Some(HistoryMessageIds::List(vec![
            i64::MAX.into(),
            "000123".into(),
            123.into(),
            "".into(),
        ])),
    };

    let result = api.client().get_mail_history(&filters).await.unwrap();
    let call = api.next().await;
    call.assert_request("/api/v2/history");
    assert_eq!(
        call.json(),
        json!({"dateFrom":"2024-02-29","dateTo":"2024-03-01","messageIds":["9223372036854775807","123"]})
    );
    assert_eq!((result.status, result.http_code), (200, 200));
    assert_eq!(result.extra["marker"], "future");
    assert!(result.extra.get("httpCode").is_none());
    let data = result.data.as_object().unwrap();
    let history = data.history.as_ref().unwrap();
    let entry = &history.outbound.as_ref().unwrap()[0];
    assert_eq!(
        entry.queue_id,
        Some(ApiInteger::String(i64::MAX.to_string()))
    );
    assert_eq!(entry.queue_id.as_ref().unwrap().as_i64(), Some(i64::MAX));
    assert_eq!(entry.status, Some(QueueStatus::Known(MailStatus::Review)));
    assert_eq!(entry.idempotency_key, Nullable::Null);
    assert!(
        matches!(entry.filter_details, Some(ObjectOrArray::Array(ref array)) if array.is_empty())
    );
    assert_eq!(entry.extra["notes"], "outbound");
    let inbound = &history.inbound.as_ref().unwrap()[0];
    assert_eq!(inbound.regard.as_deref(), Some("Subject"));
    assert_eq!(inbound.reply_to.as_ref().unwrap()[0].name, Nullable::Null);
    assert_eq!(history.extra["unknownDirection"], json!([]));
    assert!(matches!(
        data.send.queue_ids.as_ref().unwrap()[0],
        ApiInteger::Number(_)
    ));
    assert_eq!(
        data.send.queue_ids.as_ref().unwrap()[1],
        ApiInteger::String("9007199254740993".into())
    );
    assert_eq!(data.send.extra["future"], "1e400");
    assert_eq!(data.send.extra["overflown"], "-1E+400");
    assert_eq!(
        data.send.extra["9007199254740993"],
        "literal 9223372036854775807"
    );
    assert_eq!(
        data.send.extra["nested"],
        json!(["9E30", "1e+30", "-9007199254740993"])
    );
    assert_eq!(data.send.extra["escaped"], "\"9007199254740993\"");
    assert_eq!(data.send.extra["safe"], 42);
}

#[tokio::test]
async fn accepts_date_only_ids_only_and_pre_epoch_utc_dates() {
    let mut api = Server::api().await;
    let client = api.client();
    let cases = [
        (
            GetMailHistoryOptions {
                date_from: Some("0001-01-01".into()),
                ..Default::default()
            },
            json!({"dateFrom":"0001-01-01"}),
        ),
        (
            GetMailHistoryOptions {
                date_from: Some((UNIX_EPOCH - Duration::from_millis(1)).into()),
                ..Default::default()
            },
            json!({"dateFrom":"1969-12-31"}),
        ),
        (
            GetMailHistoryOptions {
                message_ids: Some(HistoryMessageIds::Single(7)),
                date_from: Some("".into()),
                ..Default::default()
            },
            json!({"messageIds":["7"]}),
        ),
        (
            GetMailHistoryOptions {
                message_ids: Some(HistoryMessageIds::Csv("123, ,00456, 123,".into())),
                ..Default::default()
            },
            json!({"messageIds":["123","456"]}),
        ),
    ];
    for (options, expected) in cases {
        client.get_mail_history(&options).await.unwrap();
        assert_eq!(api.next().await.json(), expected);
    }
}

#[tokio::test]
async fn rejects_invalid_dates_and_identifiers_locally() {
    let mut api = Server::api().await;
    let client = api.client();
    for date in [
        "0000-01-01",
        "2025-02-29",
        "2026-02-30",
        "2026-4-01",
        "2026-04-01T00:00:00Z",
        "   ",
    ] {
        let options = GetMailHistoryOptions {
            date_from: Some(date.into()),
            ..Default::default()
        };
        assert!(matches!(
            client.get_mail_history(&options).await,
            Err(Error::InvalidInput {
                field: "dateFrom",
                ..
            })
        ));
    }
    let reverse = GetMailHistoryOptions {
        date_from: Some("2026-11-01".into()),
        date_to: Some("2026-01-01".into()),
        ..Default::default()
    };
    assert!(client.get_mail_history(&reverse).await.is_err());
    let no_start = GetMailHistoryOptions {
        date_to: Some("2026-01-01".into()),
        ..Default::default()
    };
    assert!(client.get_mail_history(&no_start).await.is_err());
    for ids in ["0", "-1", "1e3", "+7", "123abc", "9223372036854775808"] {
        let options = GetMailHistoryOptions {
            message_ids: Some(HistoryMessageIds::Csv(ids.into())),
            ..Default::default()
        };
        assert!(matches!(
            client.get_mail_history(&options).await,
            Err(Error::InvalidInput {
                field: "messageId",
                ..
            })
        ));
    }
    api.assert_empty();
}

#[tokio::test]
async fn preserves_valid_array_data_and_unrecognized_object_data() {
    let api = Server::start(|_| {
        json_response(422, r#"{"status":422,"error":null,"data":["anything",{}]}"#)
    })
    .await;
    let options = GetMailHistoryOptions {
        message_ids: Some(HistoryMessageIds::Single(9)),
        ..Default::default()
    };
    let response = api.client().get_mail_history(&options).await.unwrap();
    assert_eq!(response.http_code, 422);
    assert_eq!(response.error, Some(Value::Null));
    assert!(
        matches!(response.data, ApiPayload::Array(content) if content == vec![json!("anything"), json!({})])
    );

    let api = Server::start(|_| {
        json_response(
            200,
            r#"{"status":200,"data":{"history":{"Outbound":"new representation"}}}"#,
        )
    })
    .await;
    let response = api.client().get_mail_history(&options).await.unwrap();
    assert!(
        matches!(response.data, ApiPayload::Unrecognized(object) if object["history"]["Outbound"] == "new representation")
    );
}
