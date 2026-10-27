//! Purpose: exercise generated current/legacy public operations and failure separation against wire fixtures.
//! Scope: portable request construction and decoding, with a controlled loopback transport.
//! Uses:
//! |-- rust/crates/client/src/generated/mod.rs
//! `-- rust/crates/client/src/operation.rs
//! External:
//! |-- serde_json
//! `-- url
//! Used by:
//! `-- cargo test -p opencode-client
//! Invariants: typed operation tests call the generated functions, not a copied endpoint table.
//! Contract tests: this file.
use opencode_client::{
    ClientErrorReason, current,
    errors::ApiError,
    legacy,
    operation::{Optional, Response},
};
use serde_json::json;

fn response(status: u16, value: serde_json::Value) -> Response {
    Response {
        status,
        content_type: Some("application/json; charset=utf-8".into()),
        body: serde_json::to_vec(&value).unwrap(),
    }
}
#[test]
fn typed_session_requests_encode_components_and_only_send_payload_fields() {
    let get: current::sessions::get::Input =
        serde_json::from_value(json!({"sessionID":"ses/a b?界"})).unwrap();
    let request = current::sessions::get::request(&get).unwrap();
    assert_eq!(request.path, "/api/session/ses%2Fa%20b%3F%E7%95%8C");
    assert_eq!(request.method, "GET");
    assert_eq!(request.declared_statuses, &[404, 400, 401]);
    let prompt: current::sessions::prompt::Input = serde_json::from_value(
        json!({"sessionID":"ses_test", "prompt":{"text":"Hello"}, "resume":false}),
    )
    .unwrap();
    assert_eq!(
        current::sessions::prompt::request(&prompt).unwrap().body,
        Some(json!({"prompt":{"text":"Hello"}, "resume":false}))
    );
}
#[test]
fn query_preserves_baseline_order_bracket_objects_and_falsy_values() {
    let history: current::sessions::history::Input =
        serde_json::from_value(json!({"sessionID":"ses_test","limit":1,"after":0})).unwrap();
    let request = current::sessions::history::request(&history).unwrap();
    assert_eq!(
        request
            .url(&url::Url::parse("http://localhost:3000/prefix").unwrap())
            .unwrap()
            .as_str(),
        "http://localhost:3000/api/session/ses_test/history?limit=1&after=0"
    );
    let location: current::location::get::Input =
        serde_json::from_value(json!({"location":{"directory":"/tmp/a b","workspace":"wrk_x"}}))
            .unwrap();
    let url = current::location::get::request(&location)
        .unwrap()
        .url(&url::Url::parse("http://localhost:3000").unwrap())
        .unwrap();
    assert_eq!(
        url.query_pairs().collect::<Vec<_>>(),
        vec![
            ("location[directory]".into(), "/tmp/a b".into()),
            ("location[workspace]".into(), "wrk_x".into())
        ]
    );
}
#[test]
fn declared_errors_are_typed_and_separate_from_client_failures() {
    let error = current::sessions::history::decode(response(
        404,
        json!({"_tag":"SessionNotFoundError","sessionID":"ses_missing","message":"missing"}),
    ))
    .unwrap_err();
    match error {
        ApiError::Declared { status, body } => {
            assert_eq!(status, 404);
            assert_eq!(
                serde_json::to_value(body).unwrap()["_tag"],
                "SessionNotFoundError"
            );
        }
        _ => panic!("declared error became client failure"),
    }
    match current::sessions::history::decode(response(502, json!({}))).unwrap_err() {
        ApiError::Client(error) => {
            assert_eq!(error.reason, ClientErrorReason::UnexpectedStatus);
            assert_eq!(error.status, Some(502));
        }
        _ => panic!("unexpected status became declared"),
    }
    match current::health::get::decode(Response {
        status: 200,
        content_type: Some("text/plain".into()),
        body: b"{}".to_vec(),
    })
    .unwrap_err()
    {
        ApiError::Client(error) => {
            assert_eq!(error.reason, ClientErrorReason::UnsupportedContentType)
        }
        _ => panic!("media failure became declared"),
    }
    match current::health::get::decode(Response {
        status: 200,
        content_type: Some("application/json".into()),
        body: b"{broken".to_vec(),
    })
    .unwrap_err()
    {
        ApiError::Client(error) => assert_eq!(error.reason, ClientErrorReason::MalformedResponse),
        _ => panic!("malformed JSON became declared"),
    }
}
#[test]
fn empty_responses_ignore_body_and_literal_types_reject_false_health() {
    current::sessions::interrupt::decode(Response {
        status: 204,
        content_type: None,
        body: b"not JSON".to_vec(),
    })
    .unwrap();
    assert!(current::health::get::decode(response(200, json!({"healthy":false}))).is_err());
    assert!(current::health::get::decode(response(200, json!({"healthy":true}))).is_ok());
}
#[test]
fn current_unwraps_data_but_legacy_preserves_its_wire_body() {
    let output = current::sessions::active::decode(response(200, json!({"data":{}}))).unwrap();
    assert_eq!(serde_json::to_value(output).unwrap(), json!({}));
    assert!(legacy::session::abort::decode(response(200, json!(true))).unwrap());
}
#[test]
fn omitted_generator_operations_and_cross_group_legacy_operations_are_public() {
    assert_eq!(current::files::read::NAME, "fs.read");
    assert_eq!(current::ptys::connect::NAME, "pty.connect");
    assert_eq!(current::ptys::connect_token::NAME, "pty.connectToken");
    assert_eq!(legacy::session::part_delete::NAME, "part.delete");
    assert!(current::operations().count() > 50);
    assert!(legacy::operations().count() > 90);
}
#[test]
fn optional_null_and_absence_remain_distinct() {
    #[derive(Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    struct Input {
        #[serde(default, skip_serializing_if = "Optional::is_missing")]
        value: Optional<Option<String>>,
    }
    assert_eq!(
        serde_json::from_value::<Input>(json!({})).unwrap().value,
        Optional::Missing
    );
    assert_eq!(
        serde_json::from_value::<Input>(json!({"value":null}))
            .unwrap()
            .value,
        Optional::Present(None)
    );
    assert_eq!(
        serde_json::to_value(Input {
            value: Optional::Present(None)
        })
        .unwrap(),
        json!({"value":null})
    );
}

#[test]
fn variadic_tuple_requires_prefix_and_roundtrips_rest_events() {
    assert!(serde_json::from_value::<legacy::sync::replay::Input>(json!({"events":[]})).is_err());
    let event = json!({"id":"evt_1","aggregateID":"ses_1","seq":0,"type":"created","data":{}});
    let input: legacy::sync::replay::Input =
        serde_json::from_value(json!({"events":[event,event]})).unwrap();
    assert_eq!(
        legacy::sync::replay::request(&input).unwrap().body,
        Some(json!({"events":[event,event]}))
    );
}

#[test]
fn wildcard_file_operation_encodes_segments_and_decodes_raw_bytes() {
    let input: current::files::read::Input =
        serde_json::from_value(json!({"wildcard":"src/a b/界.rs"})).unwrap();
    assert_eq!(
        current::files::read::request(&input).unwrap().path,
        "/api/fs/read/src/a%20b/%E7%95%8C.rs"
    );
    assert_eq!(
        current::files::read::decode(Response {
            status: 200,
            content_type: Some("application/octet-stream".into()),
            body: vec![0, 255, 1]
        })
        .unwrap(),
        vec![0, 255, 1]
    );
}
